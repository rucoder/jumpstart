FROM ubuntu:22.04 as base-build

RUN apt-get update && apt-get install -y \
    build-essential \
    git \
    nasm \
    python3 \
    python3-pip \
    uuid-dev \
    iasl

# make a link to python3
RUN ln -s /usr/bin/python3 /usr/bin/python

ADD --keep-git-dir=true https://github.com/tianocore/edk2.git#edk2-stable202402 /edk2

WORKDIR /edk2

RUN git submodule update --init --recursive

RUN make -j $(nproc) -C BaseTools

FROM base-build as build-ovmf
WORKDIR /edk2
RUN bash -c "source edksetup.sh; build -n $(nproc) -b RELEASE -a X64 -t GCC5 -p OvmfPkg/OvmfPkgX64.dsc"

FROM base-build as build-ovmf-no-nvme
WORKDIR /edk2
# disable NVME support
RUN sed -i 's/.*NvmExpressDxe.*/# disable NVME/' OvmfPkg/OvmfPkgX64.dsc && \
    sed -i 's/.*NvmExpressDxe.*/# disable NVME/' OvmfPkg/OvmfPkgX64.fdf
RUN bash -c "source edksetup.sh; build -n $(nproc) -b RELEASE -a X64 -t GCC5 -p OvmfPkg/OvmfPkgX64.dsc"

FROM base-build as build-shell
WORKDIR /edk2
RUN bash -c "source edksetup.sh; build -n $(nproc) -b RELEASE -a X64 -t GCC5 -p ShellPkg/ShellPkg.dsc"

FROM scratch as ovmf
# get NVME DXE driver
COPY --from=build-ovmf /edk2/Build/OvmfX64/RELEASE_GCC5/X64/NvmExpressDxe.efi /NvmExpressDxe.efi
# get OVMF firmware without NVME support
COPY --from=build-ovmf-no-nvme /edk2/Build/OvmfX64/RELEASE_GCC5/FV/OVMF.fd /OVMF_no_nvme.fd
COPY --from=build-ovmf-no-nvme /edk2/Build/OvmfX64/RELEASE_GCC5/FV/OVMF_VARS.fd /OVMF_VARS_no_nvme.fd
COPY --from=build-ovmf-no-nvme /edk2/Build/OvmfX64/RELEASE_GCC5/FV/OVMF_CODE.fd /OVMF_CODE_no_nvme.fd
# get shell
COPY --from=build-shell /edk2/Build/Shell/RELEASE_GCC5/X64/ShellPkg/Application/Shell/Shell/OUTPUT/Shell.efi /shellx64.efi


