// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

pub(crate) mod cache_info;
mod fdt;
/// Module for the global interrupt controller configuration.
pub mod gic;
/// Architecture specific KVM-related code
pub mod kvm;
/// Layout for this aarch64 system.
pub mod layout;
/// Logic for configuring aarch64 registers.
pub mod regs;
/// Architecture specific vCPU code
pub mod vcpu;
/// Architecture specific VM state code
pub mod vm;

use std::cmp::min;
use std::fmt::Debug;
use std::fs::File;

use linux_loader::loader::pe::PE as Loader;
use linux_loader::loader::{Cmdline, KernelLoader};
use vm_memory::GuestMemoryError;

use crate::arch::{BootProtocol, EntryPoint};
use crate::cpu_config::aarch64::{CpuConfiguration, CpuConfigurationError};
use crate::cpu_config::templates::CustomCpuTemplate;
use crate::initrd::InitrdConfig;
use crate::vmm_config::machine_config::MachineConfig;
use crate::vstate::memory::{Address, Bytes, GuestAddress, GuestMemory, GuestMemoryMmap};
use crate::vstate::vcpu::KvmVcpuError;
use crate::{Vcpu, VcpuConfig, Vmm};

/// Errors thrown while configuring aarch64 system.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum ConfigurationError {
    /// Failed to create a Flattened Device Tree for this aarch64 microVM: {0}
    SetupFDT(#[from] fdt::FdtError),
    /// Failed to write to guest memory.
    MemoryError(GuestMemoryError),
    /// Cannot copy kernel file fd
    KernelFile,
    /// Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}
    KernelLoader(#[from] linux_loader::loader::Error),
    /// Error creating vcpu configuration: {0}
    VcpuConfig(#[from] CpuConfigurationError),
    /// Error configuring the vcpu: {0}
    VcpuConfigure(KvmVcpuError),
}

/// The start of the memory area reserved for MMIO devices.
pub const MMIO_MEM_START: u64 = layout::MAPPED_IO_START;
/// The size of the memory area reserved for MMIO devices.
pub const MMIO_MEM_SIZE: u64 = layout::DRAM_MEM_START - layout::MAPPED_IO_START; //>> 1GB

/// Returns a Vec of the valid memory addresses for aarch64.
/// See [`layout`](layout) module for a drawing of the specific memory model for this platform.
pub fn arch_memory_regions(size: usize) -> Vec<(GuestAddress, usize)> {
    let dram_size = min(size, layout::DRAM_MEM_MAX_SIZE);
    vec![(GuestAddress(layout::DRAM_MEM_START), dram_size)]
}

/// Configures the system for booting Linux.
pub fn configure_system_for_boot(
    vmm: &mut Vmm,
    vcpus: &mut [Vcpu],
    machine_config: &MachineConfig,
    cpu_template: &CustomCpuTemplate,
    entry_point: EntryPoint,
    initrd: &Option<InitrdConfig>,
    boot_cmdline: Cmdline,
) -> Result<(), ConfigurationError> {
    // Construct the base CpuConfiguration to apply CPU template onto.
    let cpu_config = CpuConfiguration::new(cpu_template, vcpus)?;

    // Apply CPU template to the base CpuConfiguration.
    let cpu_config = CpuConfiguration::apply_template(cpu_config, cpu_template);

    let vcpu_config = VcpuConfig {
        vcpu_count: machine_config.vcpu_count,
        smt: machine_config.smt,
        cpu_config,
    };

    let optional_capabilities = vmm.kvm.optional_capabilities();
    // Configure vCPUs with normalizing and setting the generated CPU configuration.
    for vcpu in vcpus.iter_mut() {
        vcpu.kvm_vcpu
            .configure(
                &vmm.guest_memory,
                entry_point,
                &vcpu_config,
                &optional_capabilities,
            )
            .map_err(ConfigurationError::VcpuConfigure)?;
    }
    let vcpu_mpidr = vcpus
        .iter_mut()
        .map(|cpu| cpu.kvm_vcpu.get_mpidr())
        .collect();
    let cmdline = boot_cmdline
        .as_cstring()
        .expect("Cannot create cstring from cmdline string");

    let fdt = fdt::create_fdt(
        &vmm.guest_memory,
        vcpu_mpidr,
        cmdline,
        vmm.mmio_device_manager.get_device_info(),
        vmm.vm.get_irqchip(),
        &vmm.acpi_device_manager.vmgenid,
        initrd,
    )?;

    let fdt_address = GuestAddress(get_fdt_addr(&vmm.guest_memory));
    vmm.guest_memory
        .write_slice(fdt.as_slice(), fdt_address)
        .map_err(ConfigurationError::MemoryError)?;

    Ok(())
}

/// Returns the memory address where the kernel could be loaded.
pub fn get_kernel_start() -> u64 {
    layout::SYSTEM_MEM_START + layout::SYSTEM_MEM_SIZE
}

/// Returns the memory address where the initrd could be loaded.
pub fn initrd_load_addr(guest_mem: &GuestMemoryMmap, initrd_size: usize) -> Option<u64> {
    let round_to_pagesize =
        |size| (size + (super::GUEST_PAGE_SIZE - 1)) & !(super::GUEST_PAGE_SIZE - 1);
    match GuestAddress(get_fdt_addr(guest_mem)).checked_sub(round_to_pagesize(initrd_size) as u64) {
        Some(offset) => {
            if guest_mem.address_in_range(offset) {
                Some(offset.raw_value())
            } else {
                None
            }
        }
        None => None,
    }
}

// Auxiliary function to get the address where the device tree blob is loaded.
fn get_fdt_addr(mem: &GuestMemoryMmap) -> u64 {
    // If the memory allocated is smaller than the size allocated for the FDT,
    // we return the start of the DRAM so that
    // we allow the code to try and load the FDT.

    if let Some(addr) = mem.last_addr().checked_sub(layout::FDT_MAX_SIZE as u64 - 1) {
        if mem.address_in_range(addr) {
            return addr.raw_value();
        }
    }

    layout::DRAM_MEM_START
}

/// Load linux kernel into guest memory.
pub fn load_kernel(
    kernel: &File,
    guest_memory: &GuestMemoryMmap,
) -> Result<EntryPoint, ConfigurationError> {
    // Need to clone the File because reading from it
    // mutates it.
    let mut kernel_file = kernel
        .try_clone()
        .map_err(|_| ConfigurationError::KernelFile)?;

    let entry_addr = Loader::load(
        guest_memory,
        Some(GuestAddress(get_kernel_start())),
        &mut kernel_file,
        None,
    )?;

    Ok(EntryPoint {
        entry_addr: entry_addr.kernel_load,
        protocol: BootProtocol::LinuxBoot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::arch_mem;

    #[test]
    fn test_regions_lt_1024gb() {
        let regions = arch_memory_regions(1usize << 29);
        assert_eq!(1, regions.len());
        assert_eq!(GuestAddress(super::layout::DRAM_MEM_START), regions[0].0);
        assert_eq!(1usize << 29, regions[0].1);
    }

    #[test]
    fn test_regions_gt_1024gb() {
        let regions = arch_memory_regions(1usize << 41);
        assert_eq!(1, regions.len());
        assert_eq!(GuestAddress(super::layout::DRAM_MEM_START), regions[0].0);
        assert_eq!(super::layout::DRAM_MEM_MAX_SIZE, regions[0].1);
    }

    #[test]
    fn test_get_fdt_addr() {
        let mem = arch_mem(layout::FDT_MAX_SIZE - 0x1000);
        assert_eq!(get_fdt_addr(&mem), layout::DRAM_MEM_START);

        let mem = arch_mem(layout::FDT_MAX_SIZE);
        assert_eq!(get_fdt_addr(&mem), layout::DRAM_MEM_START);

        let mem = arch_mem(layout::FDT_MAX_SIZE + 0x1000);
        assert_eq!(get_fdt_addr(&mem), 0x1000 + layout::DRAM_MEM_START);
    }
}
