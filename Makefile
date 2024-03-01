TARGET ?= release

OVMF_DIR = ./ovmf-no-nvme
OVMF_FILES = \
	./$(OVMF_DIR)/OVMF_CODE_no_nvme.fd \
	./$(OVMF_DIR)/OVMF_VARS_no_nvme.fd \
	./$(OVMF_DIR)/OVMF_no_nvme.fd \
	./$(OVMF_DIR)/NvmExpressDxe.efi \
	./$(OVMF_DIR)/shellx64.efi

ESP_DIR = ./vm/esp
NVME_DIR = ./vm/nvme

BOOT_FILES = \
	$(ESP_DIR)/EFI/BOOT/BOOTX64.efi \
	$(ESP_DIR)/EFI/BOOT/JS/DRIVERS/NvmExpressDxe.efi

ifeq ($(TARGET),debug)
	BOOT_FILES += $(ESP_DIR)/EFI/BOOT/shellx64.efi
endif

BOOTLOADER = ./target/x86_64-unknown-uefi/$(TARGET)/jumpstart.efi

.PHONY: all
all: run

.PHONY: ovmf
ovmf: $(OVMF_FILES)
$(OVMF_FILES): Dockerfile
	docker buildx build  -o type=local,dest=$(OVMF_DIR) .

.PHONY: vm-dir
vm-dir: $(BOOT_FILES)
$(BOOT_FILES): jumpstart_$(TARGET) ovmf
	mkdir -p $(ESP_DIR)/EFI/BOOT/JS/DRIVERS
	mkdir -p $(NVME_DIR)
	touch $(NVME_DIR)/nvme-dummy.txt
	cp $(OVMF_DIR)/NvmExpressDxe.efi $(ESP_DIR)/EFI/BOOT/JS/DRIVERS
	cp $(BOOTLOADER) $(ESP_DIR)/EFI/BOOT/BOOTX64.efi
# copy UEFI Shell to the ESP only for debug target
ifeq ($(TARGET),debug)
	cp $(OVMF_DIR)/shellx64.efi $(ESP_DIR)/EFI/BOOT
endif

.PHONY: jumpstart_debug jumpstart_release
jumpstart_$(TARGET): $(BOOTLOADER)

jumpstart_debug: CARGO_TARGET:=
jumpstart_release: CARGO_TARGET:=--release

$(BOOTLOADER):
	cargo build --target=x86_64-unknown-uefi $(CARGO_TARGET)

.PHONY: run
run: vm-dir
	qemu-system-x86_64 -enable-kvm -serial stdio \
	-debugcon file:debug.log -global isa-debugcon.iobase=0x402 \
    -drive if=pflash,format=raw,readonly=on,file=./$(OVMF_DIR)/OVMF_CODE_no_nvme.fd \
    -drive if=pflash,format=raw,readonly=on,file=./$(OVMF_DIR)/OVMF_VARS_no_nvme.fd \
	-drive format=raw,file=fat:rw:$(NVME_DIR),if=none,id=nvm \
	-device nvme,serial=deadbeef,drive=nvm \
    -drive format=raw,file=fat:rw:$(ESP_DIR)

.PHONY: clean
clean:
	rm -rf $(OVMF_DIR)
	rm -rf ./vm
	rm debug.log