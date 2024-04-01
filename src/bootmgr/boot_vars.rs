extern crate alloc;

use core::{fmt::Display, mem::size_of};

use alloc::{boxed::Box, format, string::ToString, vec::Vec};
use anyhow::{anyhow, Context, Result};
use log::info;
use regex::*;
use uefi::{
    cstr16,
    proto::device_path::{
        text::{AllowShortcuts, DisplayOnly},
        DevicePath,
    },
    table::runtime::{RuntimeServices, VariableVendor},
    CString16, Char16, Status,
};

#[derive(Debug)]
enum LoadOptionAttributesBits {
    LoadOptionActive = 0x00000001,
    LoadOptionForceReconnect = 0x00000002,
    LoadOptionHidden = 0x00000008,
    LoadOptionCategory = 0x00001F00,
    LoadOptionCategoryApp = 0x000000100,
    LoadOptionCategoryBoot = 0x000000000,
}
#[derive(Debug)]
struct LoadOptionAttributes(u32);

impl LoadOptionAttributes {
    fn from(data: u32) -> Self {
        LoadOptionAttributes(data)
    }
    fn is_active(&self) -> bool {
        self.0 & LoadOptionAttributesBits::LoadOptionActive as u32 != 0
    }
    fn is_force_reconnect(&self) -> bool {
        self.0 & LoadOptionAttributesBits::LoadOptionForceReconnect as u32 != 0
    }
    fn is_hidden(&self) -> bool {
        self.0 & LoadOptionAttributesBits::LoadOptionHidden as u32 != 0
    }
    fn category(&self) -> u32 {
        self.0 & LoadOptionAttributesBits::LoadOptionCategory as u32
    }
    fn is_category_app(&self) -> bool {
        self.0 & LoadOptionAttributesBits::LoadOptionCategoryApp as u32 != 0
    }
    fn is_category_boot(&self) -> bool {
        self.0 & LoadOptionAttributesBits::LoadOptionCategoryBoot as u32 != 0
    }
}

impl From<LoadOptionAttributes> for u32 {
    fn from(data: LoadOptionAttributes) -> Self {
        data.0
    }
}

impl Display for LoadOptionAttributes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "LoadOptionAttributes {{ active: {}, force_reconnect: {}, hidden: {}, category: {}, category_app: {}, category_boot: {} }}",
            self.is_active(),
            self.is_force_reconnect(),
            self.is_hidden(),
            self.category(),
            self.is_category_app(),
            self.is_category_boot()
        )
    }
}

pub struct EfiLoadOption {
    pub attributes: LoadOptionAttributes,
    pub description: CString16,
    pub device_path_list: Vec<Box<DevicePath>>,
    pub optional_data: Option<Vec<u8>>,
}

trait TryFromNeBytes: Sized {
    fn try_from_ne_bytes(data: &[u8]) -> Result<Self>;
}

impl TryFromNeBytes for CString16 {
    fn try_from_ne_bytes(data: &[u8]) -> Result<Self> {
        let data_len = data.len() & !1;
        let mut ret = CString16::new();

        let mut index = 0;
        while index < data_len {
            let c = u16::from_ne_bytes([data[index], data[index + 1]]);
            index += 2;
            if c == 0 {
                return Ok(ret);
            }
            // pushing of NULL character is invalid
            ret.push(Char16::try_from(c).map_err(anyhow::Error::msg)?);
        }
        Err(anyhow!("CString16 not null-terminated"))
    }
}

impl TryFrom<&[u8]> for EfiLoadOption {
    type Error = anyhow::Error;
    fn try_from(data: &[u8]) -> Result<Self> {
        // info!("Data length: {:?} Data: {:?}", data.len(), data);

        if data.len() < 6 {
            return Err(anyhow!("EfiBootOption data too short"));
        }

        let (attribute_bytes, data) = data.split_at(size_of::<u32>());
        let attributes =
            u32::from_ne_bytes(attribute_bytes.try_into().map_err(anyhow::Error::msg)?);

        let (file_path_list_length_bytes, data) = data.split_at(size_of::<u16>());
        let mut file_path_list_length = u16::from_ne_bytes(
            file_path_list_length_bytes
                .try_into()
                .map_err(anyhow::Error::msg)?,
        ) as usize;

        // if file_path_list_length > data.len() {
        //     return Err(anyhow!("EfiBootOption file_path_list_length too long"));
        // }
        // if file_path_list_length == 0 {
        //     return Err(anyhow!("EfiBootOption file_path_list_length is zero"));
        // }
        // if file_path_list_length % 2 != 0 {
        //     return Err(anyhow!("EfiBootOption file_path_list_length is not even"));
        // }

        // info!(
        //     "Attributes: {:?}, file_path_list_length: {}",
        //     attributes, file_path_list_length
        // );

        // let header_length = size_of::<u32>() + size_of::<u16>();

        //TODO: set limit of bytes to process
        let description = CString16::try_from_ne_bytes(&data)?;

        // let optional_data_offset = header_length + description.num_bytes() + file_path_list_length;

        // if optional_data_offset > data.len() {
        //     return Err(anyhow!("EfiBootOption optional_data_offset too large"));
        // }

        // let optional_data = if data.len() > optional_data_offset {

        //     Some(data[optional_data_offset..].to_vec())
        // } else {
        //     None
        // };

        let mut device_path_list: Vec<_> = Vec::new();
        // info!("Data length: {:?} Data: {:?}", data.len(), data);

        let (_, mut data) = data.split_at(description.num_bytes());

        // info!("Data length: {:?} Data: {:?}", data.len(), data);

        loop {
            let device_path =
                unsafe { DevicePath::from_ffi_ptr(data.as_ptr() as *mut _) }.to_boxed();
            let device_path_len = device_path.as_bytes().len();
            device_path_list.push(device_path);

            file_path_list_length = file_path_list_length
                .checked_sub(device_path_len)
                .context("file_path_list_length underflow")?;

            (_, data) = data.split_at(device_path_len);
            // info!("Data length: {:?} Data: {:?}", data.len(), data);

            if file_path_list_length == 0 {
                break;
            }
        }

        // info!("Data length 1: {:?} Data: {:?}", data.len(), data);

        let optional_data = if !data.is_empty() {
            Some(data.to_vec())
        } else {
            None
        };

        // let mut defice_path_offset = header_length + description.num_bytes();
        // let mut device_path_data = &data[defice_path_offset..];

        // while file_path_list_length > 0 {
        //     let device_path =
        //         unsafe { DevicePath::from_ffi_ptr(device_path_data.as_ptr() as *mut _) }.to_boxed();

        //     info!("Device path: {:?}", device_path);

        //     let device_path_len = device_path.as_bytes().len();
        //     device_path_list.push(device_path);
        //     file_path_list_length -= device_path_len;
        //     if file_path_list_length > 0 {
        //         defice_path_offset += device_path_len;
        //         device_path_data = &data[defice_path_offset..];
        //     }
        // }

        // info!("Device path list: {:?}", device_path_list);

        Ok(EfiLoadOption {
            attributes: LoadOptionAttributes::from(attributes),
            description,
            device_path_list,
            optional_data,
        })

        // let mut data = data;
        // info!("Data length: {:?}", data.len());
        // if data.len() < 6 {
        //     return Err(anyhow!("EfiBootOption data too short"));
        // }

        // let attributes = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        // let mut file_path_list_length = u16::from_ne_bytes([data[4], data[5]]) as usize;
        // let header_length = size_of::<u32>() + size_of::<u16>();
        // let description = CString16::try_from_ne_bytes(&data[header_length..])?;
        // let mut device_path_list = Vec::new();

        // data = &data[header_length + description.num_bytes()..];
        // // Now create a DevicePath from the rest of the data
        // while file_path_list_length > 0 {
        //     info!("list length: {}", file_path_list_length);
        //     let device_path =
        //         unsafe { DevicePath::from_ffi_ptr(data.as_ptr() as *mut _) }.to_boxed();
        //     let device_path_len = device_path.as_bytes().len();
        //     info!("Device path length: {:?}", device_path_len);

        //     device_path_list.push(device_path);

        //     data = &data[device_path_len..];
        //     file_path_list_length -= device_path_len;
        // }

        // // info!("Data: {:?}", data);
        // // info!("Data len: {}", data.len());
        // // info!("Device path len: {}", devcie_path_len);

        // // And optional data if something is left
        // let optional_data = if data.len() > 0 {
        //     // let d = CString16::try_from_ne_bytes(&data[devcie_path_len..])?;
        //     // info!("Optional data: {:?}", d);
        //     Some(data.to_vec())
        // } else {
        //     None
        // };

        // info!("Optional data: {:?}", optional_data);
    }
}

impl Display for EfiLoadOption {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{{ attributes: {}, description: {}, device_path: {:?}, optional_data: {:?} }}",
            self.attributes,
            self.description,
            self.device_path_list
                .iter()
                .map(|p| p
                    .to_string(
                        uefi_services::system_table().boot_services(),
                        DisplayOnly(true),
                        AllowShortcuts(false)
                    )
                    .unwrap()
                    .to_string())
                .collect::<Vec<_>>(),
            self.optional_data
        )
    }
}

impl From<EfiLoadOption> for Vec<u8> {
    fn from(data: EfiLoadOption) -> Vec<u8> {
        let mut v = Vec::new();
        let file_path_list_length: u16 =
            data.device_path_list
                .iter()
                .fold(0, |acc, p| acc + p.as_bytes().len()) as u16;

        v.extend_from_slice(&u32::from(data.attributes).to_ne_bytes());
        v.extend_from_slice(&file_path_list_length.to_ne_bytes());
        v.extend_from_slice(data.description.as_bytes());

        for device_path in data.device_path_list {
            v.extend_from_slice(device_path.as_bytes());
        }

        if let Some(optional_data) = data.optional_data {
            v.extend_from_slice(&optional_data);
        }
        v
    }
}
#[derive(Debug)]
pub struct EfiBootOrder {
    pub boot_order: Vec<u16>,
}

impl TryFrom<&[u8]> for EfiBootOrder {
    type Error = anyhow::Error;
    fn try_from(data: &[u8]) -> Result<Self> {
        let mut boot_order = Vec::new();
        let mut data = data;
        //FIXME: need to check if data is even
        while !data.is_empty() {
            let order = u16::from_ne_bytes([data[0], data[1]]);
            boot_order.push(order);
            data = &data[2..];
        }
        Ok(EfiBootOrder { boot_order })
    }
}

impl EfiBootOrder {
    pub fn as_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        for order in &self.boot_order {
            v.extend_from_slice(&order.to_ne_bytes());
        }
        v
    }
    // fn read_boot_option(&self, index: usize) -> Result<EfiBootOption> {
    //     let boot_option = self.boot_order.get(index).ok_or_else(|| {
    //         anyhow!(
    //             "EfiBootOrder index out of range: {} (max: {})",
    //             index,
    //             self.boot_order.len()
    //         )
    //     })?;
    //     let (value, _) = uefi::table::runtime::get_variable(
    //         cstr16!("Boot{:04X}").as_ref(),
    //         &VariableVendor::GLOBAL_VARIABLE,
    //     )
    //     .map_err(anyhow::Error::msg)?;
    //     let boot_option = EfiBootOption::try_from(value.as_ref())?;
    //     Ok(boot_option)
    // }
    // fn write_boot_option(&mut self, index: usize, boot_option: EfiBootOption) -> Result<()> {
    //     let boot_option = Vec::from(boot_option);
    //     let boot_option = CString16::from(boot_option);
    //     let boot_option = boot_option.as_bytes();
    //     let boot_order = self.as_bytes();
    //     let (status, _) = uefi::table::runtime::set_variable(
    //         cstr16!("BootOrder").as_ref(),
    //         &VariableVendor::GLOBAL_VARIABLE,
    //         boot_order.as_slice(),
    //     )
    //     .map_err(anyhow::Error::msg)?;
    //     if status != Status::SUCCESS {
    //         return Err(anyhow!("failed to set BootOrder: {:?}", status));
    //     }
    //     let (status, _) = uefi::table::runtime::set_variable(
    //         cstr16!("Boot{:04X}").as_ref(),
    //         &VariableVendor::GLOBAL_VARIABLE,
    //         boot_option.as_slice(),
    //     )
    //     .map_err(anyhow::Error::msg)?;
    //     if status != Status::SUCCESS {
    //         return Err(anyhow!("failed to set Boot{:04X}: {:?}", index, status));
    //     }
    //     Ok(())
    // }

    pub fn new_from_variable(rs: &RuntimeServices) -> Result<Self> {
        let (value, _) = rs
            .get_variable_boxed(
                cstr16!("BootOrder").as_ref(),
                &VariableVendor::GLOBAL_VARIABLE,
            )
            .map_err(anyhow::Error::msg)?;
        let boot_order = EfiBootOrder::try_from(value.as_ref())?;
        Ok(boot_order)
    }
}

pub struct EfiBootManager {
    pub boot_options: Vec<(usize, EfiLoadOption)>,
    pub boot_order: EfiBootOrder,
}

impl EfiBootManager {
    pub fn new_from_variables(rs: &RuntimeServices) -> Result<Self> {
        let boot_order = EfiBootOrder::new_from_variable(rs)?;
        let mut boot_options = Vec::new();

        // try reading all boot options from variables
        let re = Regex::new(r"^Boot([0-9A-Fa-f]{4})$").unwrap();
        let var_key = rs.variable_keys().map_err(anyhow::Error::msg)?;

        for k in var_key.iter() {
            // info!("VarKey: {:?}", k.to_string());
            let var = k.name().map_err(anyhow::Error::msg)?;
            if let Some(cap) = re.captures(&var.to_string()) {
                let (value, attr) = rs
                    .get_variable_boxed(var, &VariableVendor::GLOBAL_VARIABLE)
                    .map_err(anyhow::Error::msg)?;
                let boot_option = EfiLoadOption::try_from(value.as_ref())?;
                let index = usize::from_str_radix(&cap[1], 16).map_err(anyhow::Error::msg)?;
                boot_options.push((index, boot_option));
            }
        }
        boot_options.sort_by(|(a, _), (b, _)| a.cmp(b));

        Ok(EfiBootManager {
            boot_options,
            boot_order,
        })
    }

    pub fn get_next_available_boot_index(&self) -> Result<usize> {
        // if there are no boot options, return 0
        if self.boot_options.is_empty() {
            return Ok(0);
        }
        // if the last index equals the last boot option index, return the next index
        if self.boot_options.last().unwrap().0 == self.boot_options.len() - 1 {
            return Ok(self.boot_options.len());
        }

        // if all above conditions are false, find the first available index
        let index = self
            .boot_options
            .iter()
            .enumerate()
            .find(|(i, (index, _))| *i != *index)
            .context("no available boot index found")?
            .0;
        Ok(index)
    }
}
