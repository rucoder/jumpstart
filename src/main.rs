#![no_main]
#![no_std]
mod bootmgr;

extern crate alloc;

use alloc::{borrow::ToOwned, boxed::Box, format, vec::Vec};
use anyhow::{anyhow, Context, Result};
use log::info;
use uefi::proto::{
    self,
    device_path::{
        build::{self, DevicePathBuilder},
        media::HardDrive,
        text::{AllowShortcuts, DisplayOnly},
        DevicePath, DevicePathNode, DevicePathNodeEnum, DeviceSubType, DeviceType,
        LoadedImageDevicePath,
    },
    loaded_image::LoadedImage,
    media::{block::BlockIO, disk::DiskIo, fs::SimpleFileSystem},
    ProtocolPointer,
};
use uefi::table::boot::{LoadImageSource, ScopedProtocol, SearchType};
use uefi::{prelude::*, CString16, Guid};

use bootmgr::boot_vars::EfiBootManager;

// Get the SimpleFileSystem for the current image handle
fn get_image_fs(bs: &BootServices) -> Result<ScopedProtocol<SimpleFileSystem>> {
    let fs = bs
        .get_image_file_system(bs.image_handle())
        .map_err(anyhow::Error::msg)?;
    Ok(fs)
}

// Get the DevicePath for the NVME driver
fn get_nvme_driver_device_path(bs: &BootServices) -> Result<Box<DevicePath>> {
    let loaded_image = bs
        .open_protocol_exclusive::<LoadedImage>(bs.image_handle())
        .map_err(anyhow::Error::msg)?;
    let image_device_path = bs
        .open_protocol_exclusive::<LoadedImageDevicePath>(bs.image_handle())
        .expect("failed to open LoadedImageDevicePath protocol");

    let mut buffer_vec = Vec::new();
    let mut builder = DevicePathBuilder::with_vec(&mut buffer_vec);

    for node in image_device_path.node_iter() {
        if node.full_type() == (DeviceType::MEDIA, DeviceSubType::MEDIA_FILE_PATH) {
            break;
        }
        builder = builder.push(&node).unwrap();
    }
    builder = builder
        .push(&build::media::FilePath {
            path_name: cstr16!(r"efi\boot\js\drivers\NvmExpressDxe.efi"),
        })
        .unwrap();
    Ok(builder.finalize().map_err(anyhow::Error::msg)?.to_owned())
}

// Load the NVME driver
fn load_nvme_driver(boot_services: &BootServices) -> Result<Handle> {
    let nvme_driver_device_path = get_nvme_driver_device_path(boot_services)?;
    let nvme_image_handle = boot_services
        .load_image(
            boot_services.image_handle(),
            LoadImageSource::FromDevicePath {
                device_path: &nvme_driver_device_path,
                from_boot_manager: false,
            },
        )
        .map_err(anyhow::Error::msg)?;
    //TODO: check image type. It must be driver
    boot_services
        .start_image(nvme_image_handle)
        .map_err(anyhow::Error::msg)?;
    Ok(nvme_image_handle)
}

// Get DevicePath string for the handle
fn get_device_path_cstr16(boot_services: &BootServices, handle: Handle) -> Result<CString16> {
    boot_services
        .open_protocol_exclusive::<DevicePath>(handle)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("cannot open_protocol_exclusive for {:?}", handle))?
        .to_string(boot_services, DisplayOnly(false), AllowShortcuts(false))
        .map_err(anyhow::Error::msg)
}

// Get DevicePath for the handle
fn get_device_path_boxed(bs: &BootServices, handle: Handle) -> Result<Box<DevicePath>> {
    bs.open_protocol_exclusive::<DevicePath>(handle)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to get_device_path_boxed for handle {:?}", handle))
        .map(|dpp| dpp.to_boxed())
}

// Connect all handles to a driver
fn connect_all_handles_to_driver(
    boot_services: &BootServices,
    driver_handle: Handle,
) -> Result<Vec<Handle>> {
    let mut connected_handles = Vec::new();

    info!("Connecting all handles to NVME driver");
    let handles = boot_services
        .locate_handle_buffer(SearchType::AllHandles)
        .map_err(anyhow::Error::msg)?;

    for handle in handles.iter() {
        // TODO: why Some(driver_handle) causes CPU exception?
        // it doesn't happen on QEMU. Buggy Dell firmware?
        match boot_services.connect_controller(*handle, None, None, true) {
            Ok(_) => {
                info!("Connected Handle: {:?}", handle);
                connected_handles.push(*handle);
            }
            Err(e) => {
                // ignore NOT_FOUND error
                if e.status() != Status::NOT_FOUND {
                    return Err(anyhow::anyhow!(e).context("failed to connect controller"));
                }
            }
        }
    }
    info!("[ DONE ] ");
    Ok(connected_handles)
}

fn get_nvme_fs_device_paths(bs: &BootServices) -> Result<Vec<Box<DevicePath>>> {
    let all_paths = get_all_device_paths_for_protocol::<SimpleFileSystem>(bs)?;
    let nvme_paths = all_paths
        .iter()
        .filter(|p| p.is_nvme())
        .map(|p| p.to_boxed())
        .collect();
    Ok(nvme_paths)
}

fn get_all_block_device_paths(bs: &BootServices) -> Result<Vec<Box<DevicePath>>> {
    get_all_device_paths_for_protocol::<BlockIO>(bs)
}

fn get_all_disk_device_paths(bs: &BootServices) -> Result<Vec<Box<DevicePath>>> {
    get_all_device_paths_for_protocol::<DiskIo>(bs)
}

// Template function for getting all device paths for a protocol
// if a device path is not found, it is ignored
fn get_all_device_paths_for_protocol<P>(bs: &BootServices) -> Result<Vec<Box<DevicePath>>>
where
    P: ProtocolPointer + ?Sized,
{
    Ok(get_all_handles_for_protocol(bs, &P::GUID)?
        .iter()
        .filter_map(|h| get_device_path_boxed(bs, *h).ok())
        .collect())
}

fn get_all_handles_for_protocol(bs: &BootServices, protocol: &Guid) -> Result<Vec<Handle>> {
    Ok(bs
        .locate_handle_buffer(SearchType::ByProtocol(protocol))
        .map_err(anyhow::Error::msg)?
        .iter()
        .copied()
        .collect())
}

trait DevicePathExt {
    fn file_path(&self) -> Option<&proto::device_path::media::FilePath>;
    fn hard_drive(&self) -> Option<&HardDrive>;
    fn is_nvme(&self) -> bool;
}

impl DevicePathExt for DevicePath {
    fn file_path(&self) -> Option<&proto::device_path::media::FilePath> {
        for inst in self.instance_iter() {
            for node in inst.node_iter() {
                if node.full_type() == (DeviceType::MEDIA, DeviceSubType::MEDIA_FILE_PATH) {
                    let e_node = node.as_enum().unwrap();
                    if let DevicePathNodeEnum::MediaFilePath(n) = e_node {
                        return Some(n);
                    }
                }
            }
        }
        None
    }

    fn hard_drive(&self) -> Option<&HardDrive> {
        for inst in self.instance_iter() {
            for node in inst.node_iter() {
                if node.full_type() == (DeviceType::MEDIA, DeviceSubType::MEDIA_HARD_DRIVE) {
                    let e_node = node.as_enum().unwrap();
                    if let DevicePathNodeEnum::MediaHardDrive(n) = e_node {
                        return Some(n);
                    }
                }
            }
        }
        None
    }

    fn is_nvme(&self) -> bool {
        for inst in self.instance_iter() {
            for node in inst.node_iter() {
                if node.full_type()
                    == (
                        DeviceType::MESSAGING,
                        DeviceSubType::MESSAGING_NVME_NAMESPACE,
                    )
                {
                    return true;
                }
            }
        }
        false
    }
}

trait PartialEqExt {
    fn eq(&self, other: &Self) -> bool;
}

impl PartialEqExt for HardDrive {
    fn eq(&self, other: &Self) -> bool {
        self.partition_number() == other.partition_number()
            && self.partition_start() == other.partition_start()
            && self.partition_size() == other.partition_size()
            && self.partition_format() == other.partition_format()
            && self.partition_number() == other.partition_number()
            && self.partition_signature() == other.partition_signature()
    }
}

trait AsBuildNode {
    fn as_media_file_path(&self) -> Result<CString16>;
}

impl AsBuildNode for DevicePathNodeEnum<'_> {
    fn as_media_file_path(&self) -> Result<CString16> {
        match self {
            DevicePathNodeEnum::MediaFilePath(p) => {
                let cstr16 = CString16::try_from(&p.path_name()).map_err(anyhow::Error::msg)?;
                Ok(cstr16)
            }
            _ => Err(anyhow!("not a MediaFilePath")),
        }
    }
}

impl AsBuildNode for &DevicePathNodeEnum<'_> {
    fn as_media_file_path(&self) -> Result<CString16> {
        (*self).as_media_file_path()
    }
}

impl AsBuildNode for DevicePathNode {
    fn as_media_file_path(&self) -> Result<CString16> {
        // FIXME: we should convert an error here to anyhow::Error
        // but NodeConversionError doesn't implement Display trait
        self.as_enum().unwrap().as_media_file_path()
    }
}

fn run_jumpstarter(bs: &BootServices, rs: &RuntimeServices) -> Result<()> {
    let nvme_driver_handle = load_nvme_driver(bs)?;

    let _connected_handles = connect_all_handles_to_driver(bs, nvme_driver_handle)?;

    // after connecting all handles to the driver, we should be able to get a simple filesystem
    // for the NVMe device
    let fs_device_paths = get_nvme_fs_device_paths(bs)?;
    for path in fs_device_paths.iter() {
        info!(
            "FS Device Path: {}",
            path.to_string(bs, DisplayOnly(false), AllowShortcuts(false))
                .map_err(anyhow::Error::msg)?
        );
        info!("HardDrive: {:?}", path.hard_drive());
        info!("FilePath: {:?}", path.file_path());
        info!("Is NVMe: {}", path.is_nvme());
    }

    let boot_mgr = EfiBootManager::new_from_variables(rs)?;

    for (index, boot_option) in boot_mgr.boot_options.iter() {
        info!("Boot{:04X}:", index);
        for (i, p) in boot_option.device_path_list.iter().enumerate() {
            let s = p
                .to_string(bs, DisplayOnly(false), AllowShortcuts(false))
                .map_err(anyhow::Error::msg)?;
            info!("Segment {}: '{}'", i, s);
            // get HardDrive from the device path
            if let Some(hd) = p.hard_drive() {
                info!("HardDrive: {:#?}", hd);
                // compare the HardDrive with the HardDrive from NVMe device paths
                for nvme_path in fs_device_paths.iter() {
                    if let Some(nvme_hd) = nvme_path.hard_drive() {
                        if hd.eq(nvme_hd) {
                            // info!("Matched NVMe Device Path: {}", s);
                            // construct a new device path with the NVMe device path prepended
                            let mut backing_vector: Vec<u8> = Vec::new();
                            let mut new_device_path =
                                DevicePathBuilder::with_vec(&mut backing_vector);
                            // push the NVMe device path
                            for node in nvme_path.node_iter() {
                                new_device_path = new_device_path.push(&node).unwrap();
                            }
                            // get the file path from the boot option
                            if let Some(file_path) = p
                                .node_iter()
                                .find(|n| {
                                    n.full_type()
                                        == (DeviceType::MEDIA, DeviceSubType::MEDIA_FILE_PATH)
                                })
                                .and_then(|e| e.as_media_file_path().ok())
                            {
                                new_device_path = new_device_path
                                    .push(&build::media::FilePath {
                                        path_name: &file_path,
                                    })
                                    .map_err(anyhow::Error::msg)?;
                            }
                            let new_device_path =
                                new_device_path.finalize().map_err(anyhow::Error::msg)?;

                            info!(
                                "We'll load this image: {}",
                                new_device_path
                                    .to_string(bs, DisplayOnly(false), AllowShortcuts(false))
                                    .map_err(anyhow::Error::msg)?
                            );

                            // load the image
                            info!("Loading image....");
                            let image_handle = bs
                                .load_image(
                                    bs.image_handle(),
                                    LoadImageSource::FromDevicePath {
                                        device_path: &new_device_path,
                                        from_boot_manager: true,
                                    },
                                )
                                .map_err(anyhow::Error::msg)?;

                            // start the image
                            info!("Starting image....");
                            bs.start_image(image_handle)
                                .map_err(anyhow::Error::msg)
                                .expect("Error starting image");
                        }
                    }
                }
            }
        }
    }

    // info!("BootOrder: {:?}", boot_mgr.boot_order);
    // info!(
    //     "Next available boot index: {}",
    //     boot_mgr.get_next_available_boot_index()?
    // );

    // // get_fs_device_paths(bs)?;

    // // print all block device paths
    // let block_device_paths = get_all_block_device_paths(bs)?;
    // for path in block_device_paths.iter() {
    //     info!(
    //         "Block Device Path: {}",
    //         path.to_string(&bs, DisplayOnly(false), AllowShortcuts(false))
    //             .map_err(anyhow::Error::msg)?
    //     );
    // }

    // // print all disk device paths
    // let disk_device_paths = get_all_disk_device_paths(bs)?;
    // for path in disk_device_paths.iter() {
    //     info!(
    //         "Disk Device Path: {}",
    //         path.to_string(&bs, DisplayOnly(false), AllowShortcuts(false))
    //             .map_err(anyhow::Error::msg)?
    //     );
    // }

    // [ INFO]:  src/main.rs@358: Next available boot index: 5
    // [ INFO]:  src/main.rs@368: Block Device Path: PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)/Ctrl(0x0)/Scsi(0x0,0x0)
    // [ INFO]:  src/main.rs@368: Block Device Path: PciRoot(0x0)/Pci(0x1A,0x0)/USB(0x0,0x0)/USB(0x5,0x0)/USB(0x3,0x0)/Unit(0x0)
    // [ INFO]:  src/main.rs@368: Block Device Path: PciRoot(0x0)/Pci(0x1A,0x0)/USB(0x0,0x0)/USB(0x5,0x0)/USB(0x3,0x0)/Unit(0x1)
    // [ INFO]:  src/main.rs@368: Block Device Path: PciRoot(0x0)/Pci(0x2,0x0)/Pci(0x0,0x0)/NVMe(0x1,0E-00-00-B0-81-A7-79-64)
    // [ INFO]:  src/main.rs@378: Disk Device Path: PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)/Ctrl(0x0)/Scsi(0x0,0x0)
    // [ INFO]:  src/main.rs@378: Disk Device Path: PciRoot(0x0)/Pci(0x1A,0x0)/USB(0x0,0x0)/USB(0x5,0x0)/USB(0x3,0x0)/Unit(0x0)
    // [ INFO]:  src/main.rs@378: Disk Device Path: PciRoot(0x0)/Pci(0x1A,0x0)/USB(0x0,0x0)/USB(0x5,0x0)/USB(0x3,0x0)/Unit(0x1)
    // [ INFO]:  src/main.rs@378: Disk Device Path: PciRoot(0x0)/Pci(0x2,0x0)/Pci(0x0,0x0)/NVMe(0x1,0E-00-00-B0-81-A7-79-64)

    Ok(())
}

#[entry]
fn main(image_handle: Handle, mut system_table: SystemTable<Boot>) -> Status {
    uefi_services::init(&mut system_table).unwrap();
    let bs = system_table.boot_services();
    let rs = system_table.runtime_services();

    // Set watchdog timer. This is not required for normal operation.
    // UEFI firmware should have already set the watchdog timer for 5 min.
    bs.set_watchdog_timer(600, 0x100000, None).unwrap();

    let ret = match run_jumpstarter(bs, rs) {
        Ok(_) => {
            info!("Jumpstarter completed successfully");
            Status::SUCCESS
        }
        Err(ref e) => {
            info!("Jumpstarter failed: {:?}", e);
            e.downcast_ref::<uefi::Error>()
                .map(|e| e.status())
                .unwrap_or(Status::LOAD_ERROR)
        }
    };

    bs.stall(25_000_000);
    ret
}
