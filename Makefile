TARGET ?= release

OUT_ROOT_DIR=./images

BOOT_ISO=$(OUT_ROOT_DIR)/efiboot.iso
EFI_BOOT_IMG=$(OUT_ROOT_DIR)/efiboot.img

OVMF_DIR = $(OUT_ROOT_DIR)/ovmf-no-nvme
OVMF_FILES = \
	$(OVMF_DIR)/OVMF_CODE_no_nvme.fd \
	$(OVMF_DIR)/OVMF_VARS_no_nvme.fd \
	$(OVMF_DIR)/OVMF_no_nvme.fd \
	$(OVMF_DIR)/NvmExpressDxe.efi \
	$(OVMF_DIR)/shellx64.efi

ESP_DIR = $(OUT_ROOT_DIR)/esp
NVME_DIR = $(OUT_ROOT_DIR)/nvme

BOOT_FILES = \
	$(ESP_DIR)/EFI/BOOT/BOOTX64.efi \
	$(ESP_DIR)/EFI/BOOT/JS/DRIVERS/NvmExpressDxe.efi \
	$(if $(filter debug,$(TARGET)),$(ESP_DIR)/EFI/BOOT/shellx64.efi)

BOOTLOADER = ./target/x86_64-unknown-uefi/$(TARGET)/jumpstart.efi

.PHONY: all
all: run-iso

.PHONY: ovmf
ovmf: $(OVMF_FILES)
$(OVMF_FILES): Dockerfile
	docker buildx build  -o type=local,dest=$(OVMF_DIR) .
	# files created by the docker build may be very old
	# so we need to touch them to update the timestamp
	touch -r Dockerfile $(OVMF_FILES)

$(BOOT_FILES): $(BOOTLOADER) $(OVMF_FILES)
	@echo "Populating ESP directory"
	mkdir -p $(ESP_DIR)/EFI/BOOT/JS/DRIVERS
	mkdir -p $(NVME_DIR)
	# touch $(NVME_DIR)/nvme-dummy.efi
	cp $(OVMF_DIR)/NvmExpressDxe.efi $(ESP_DIR)/EFI/BOOT/JS/DRIVERS
	cp $(BOOTLOADER) $(ESP_DIR)/EFI/BOOT/BOOTX64.efi
# copy UEFI Shell to the ESP only for debug target
ifeq ($(TARGET),debug)
	cp $(OVMF_DIR)/shellx64.efi $(ESP_DIR)/EFI/BOOT
endif

#
# Gebnerate an ISO image that can be booted from UEFI only
#
# FIXME: should I add USB boot support?
# For more information see:
# https://www.0xf8.org/2020/03/recreating-isos-that-boot-from-both-dvd-and-mass-storage-such-as-usb-sticks-and-in-both-legacy-bios-and-uefi-environments/
# https://wiki.debian.org/RepackBootableISO#What_to_do_if_no_file_.2F.disk.2Fmkisofs_exists
# https://dev.lovelyhq.com/libburnia/libisofs/raw/branch/master/doc/boot_sectors.txt
# https://fedoraproject.org/wiki/User:Pjones/BootableCDsForBIOSAndUEFI#New_UEFI.2FBIOS_hybrid_method
.PHONY: iso
iso: $(BOOT_ISO)
$(BOOT_ISO): $(EFI_BOOT_IMG)
	# create a temorray directory
	$(eval iso_tmp_dir:=$(shell mktemp -d))
	cp -r $(ESP_DIR)/* $(iso_tmp_dir)
	cp $^ $(iso_tmp_dir)/EFI/BOOT/efiboot.img
	@mkisofs \
		-o $@ \
		-R -J -v -d -N \
		-x $@ \
		-hide-rr-moved \
		-no-emul-boot \
		-eltorito-platform efi \
		-eltorito-boot EFI/BOOT/efiboot.img \
		-V "EFIBOOTISO" \
		-A "EFI Boot ISO Test"  \
		$(iso_tmp_dir)
	# cleanup
	rm -rf $(iso_tmp_dir)

$(EFI_BOOT_IMG): $(BOOT_FILES)
	# Create a FAT32 image for the UEFI boot files
	# remove image file if it already exists so we do not calculate its size
	rm -f $@
	# calculate the size of the image in megabytes
	# $(eval image_size:=$(shell du -sm $(ESP_DIR) | cut -f1))

	@echo "Creating FAT32 image with size $(image_size)"

	dd if=/dev/zero of=$@ bs=1M count=$(image_size)
	mkfs.vfat -n 'JSEFIBOOT' $@

	mmd -i $@ ::EFI
	mmd -i $@ ::EFI/BOOT
	mmd -i $@ ::EFI/BOOT/JS
	mmd -i $@ ::EFI/BOOT/JS/DRIVERS

	mcopy -i $@ $(ESP_DIR)/EFI/BOOT/BOOTX64.efi ::EFI/BOOT/BOOTX64.EFI
	mcopy -i $@ $(ESP_DIR)/EFI/BOOT/JS/DRIVERS/NvmExpressDxe.efi ::EFI/BOOT/JS/DRIVERS/NvmExpressDxe.efi
ifeq ($(TARGET),debug)
	mcopy -i $@ $(ESP_DIR)/EFI/BOOT/shellx64.efi ::EFI/BOOT/SHELLX64.EFI
endif

RUST_SRC_FILES := $(shell find ./src -type f -name '*.rs')
RUST_SRC_FILES += Cargo.toml Cargo.lock rust-toolchain.toml

$(BOOTLOADER): $(RUST_SRC_FILES)
	cargo build --target=x86_64-unknown-uefi $(if $(filter release,$(TARGET)),--release)

$(OUT_ROOT_DIR)/nvme-1.img:
	dd if=/dev/zero of=$@ bs=1M count=1024

.PHONY: run-iso
run-iso: ovmf $(BOOT_ISO)
	qemu-system-x86_64 -enable-kvm -serial stdio \
	-debugcon file:debug.log -global isa-debugcon.iobase=0x402 \
	-bios $(OVMF_DIR)/OVMF_no_nvme.fd \
	-cdrom $(BOOT_ISO) -boot d -m 512

.PHONY: check-iso
check-iso: $(BOOT_ISO)
	xorriso -indev $^ -report_system_area plain -report_el_torito plain

.PHONY: run
run: $(OUT_ROOT_DIR)/nvme-1.img $(BOOT_FILES)
	qemu-system-x86_64 -enable-kvm -serial stdio \
	-debugcon file:debug.log -global isa-debugcon.iobase=0x402 \
	-drive if=pflash,format=raw,unit=0,file=$(OVMF_DIR)/OVMF_CODE_no_nvme.fd,readonly=on \
	-drive if=pflash,format=raw,unit=1,file=$(OVMF_DIR)/OVMF_VARS_no_nvme.fd \
	-drive format=raw,file=fat:rw:$(NVME_DIR),if=none,id=nvm \
	-device nvme,serial=deadbeef,drive=nvm \
	-drive format=raw,file=$<,if=none,id=nvm-1 \
	-device nvme,serial=beefdead,drive=nvm-1 \
	-drive format=raw,file=fat:rw:$(ESP_DIR)

.PHONY: clean
clean:
	cargo clean
	rm -rf $(OUT_ROOT_DIR)
	rm -f debug.log
	rm -f efiboot.iso
