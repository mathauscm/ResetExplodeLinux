//! Placa de vídeo. NVIDIA via NVML (a mesma biblioteca do app do Windows, `Nvml.cs`), carregada em tempo de execução:
//! sem driver NVIDIA o app continua funcionando, só sem esta parte. AMD/Intel entram depois pelo sysfs do amdgpu/xe.

use nvml_wrapper::bitmasks::device::ThrottleReasons;
use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState, TemperatureSensor};
use nvml_wrapper::Nvml;
use serde::Serialize;
use std::sync::OnceLock;

static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

fn nvml() -> Option<&'static Nvml> {
    NVML.get_or_init(|| Nvml::init().ok()).as_ref()
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub vendor: String,
    pub architecture: Option<String>,
    pub driver: Option<String>,
    /// Versão máxima de CUDA suportada pelo driver, ex.: "13.1".
    pub cuda: Option<String>,
    pub vbios: Option<String>,
    pub uuid: Option<String>,
    pub vram_total: Option<u64>,
    pub memory_bus_bits: Option<u32>,
    pub cuda_cores: Option<u32>,
    pub pci_bus_id: Option<String>,
    pub pci_device_id: Option<String>,
    pub pci_subsystem_id: Option<String>,
    pub pcie_max_gen: Option<u32>,
    pub pcie_max_width: Option<u32>,
    pub pcie_gen: Option<u32>,
    pub pcie_width: Option<u32>,
    pub max_core_mhz: Option<u32>,
    pub max_mem_mhz: Option<u32>,
    pub power_default_w: Option<f32>,
    pub power_min_w: Option<f32>,
    pub power_max_w: Option<f32>,
    pub fans: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GpuLive {
    pub temp_c: Option<f32>,
    pub load_pct: Option<f32>,
    pub mem_ctrl_pct: Option<f32>,
    pub core_mhz: Option<f32>,
    pub mem_mhz: Option<f32>,
    pub power_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    pub fan_pct: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub encoder_pct: Option<f32>,
    pub decoder_pct: Option<f32>,
    /// "P0".."P15"
    pub pstate: Option<String>,
    /// O que está segurando o clock agora (PerfCap do GPU-Z): "Energia", "Térmico", "Ocioso"...
    pub perfcap: Vec<String>,
}

pub fn nvidia_available() -> bool {
    nvml().is_some()
}

/// (índice, nome) das GPUs NVIDIA, lido uma vez: os sensores consultam isso a cada segundo.
pub fn names() -> &'static [(u32, String)] {
    static NAMES: OnceLock<Vec<(u32, String)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let Some(nv) = nvml() else { return Vec::new() };
        (0..nv.device_count().unwrap_or(0))
            .filter_map(|i| Some((i, nv.device_by_index(i).ok()?.name().ok()?)))
            .collect()
    })
}

pub fn list() -> Vec<GpuInfo> {
    let Some(nv) = nvml() else { return Vec::new() };
    let n = nv.device_count().unwrap_or(0);
    let driver = nv.sys_driver_version().ok();
    let cuda = nv.sys_cuda_driver_version().ok().map(|v| format!("{}.{}", v / 1000, (v % 1000) / 10));
    (0..n)
        .filter_map(|i| {
            let d = nv.device_by_index(i).ok()?;
            let pci = d.pci_info().ok();
            let pm = d.power_management_limit_constraints().ok();
            Some(GpuInfo {
                index: i,
                name: d.name().unwrap_or_else(|_| "NVIDIA".into()),
                vendor: "NVIDIA".into(),
                architecture: d.architecture().ok().map(|a| a.to_string()),
                driver: driver.clone(),
                cuda: cuda.clone(),
                vbios: d.vbios_version().ok(),
                uuid: d.uuid().ok(),
                vram_total: d.memory_info().ok().map(|m| m.total),
                memory_bus_bits: d.memory_bus_width().ok(),
                cuda_cores: d.num_cores().ok(),
                pci_bus_id: pci.as_ref().map(|p| p.bus_id.clone()),
                pci_device_id: pci.as_ref().map(|p| format!("{:04X}:{:04X}", p.pci_device_id & 0xFFFF, p.pci_device_id >> 16)),
                pci_subsystem_id: pci.as_ref().and_then(|p| p.pci_sub_system_id).map(|s| format!("{:04X}:{:04X}", s & 0xFFFF, s >> 16)),
                pcie_max_gen: d.max_pcie_link_gen().ok(),
                pcie_max_width: d.max_pcie_link_width().ok(),
                pcie_gen: d.current_pcie_link_gen().ok(),
                pcie_width: d.current_pcie_link_width().ok(),
                max_core_mhz: d.max_clock_info(Clock::Graphics).ok(),
                max_mem_mhz: d.max_clock_info(Clock::Memory).ok(),
                power_default_w: d.power_management_limit_default().ok().map(mw),
                power_min_w: pm.as_ref().map(|p| mw(p.min_limit)),
                power_max_w: pm.as_ref().map(|p| mw(p.max_limit)),
                fans: d.num_fans().ok(),
            })
        })
        .collect()
}

pub fn live(index: u32) -> Option<GpuLive> {
    let d = nvml()?.device_by_index(index).ok()?;
    let mem = d.memory_info().ok();
    let util = d.utilization_rates().ok();
    let fan = d.fan_speed(0).ok();
    Some(GpuLive {
        temp_c: d.temperature(TemperatureSensor::Gpu).ok().map(|v| v as f32),
        load_pct: util.as_ref().map(|u| u.gpu as f32),
        mem_ctrl_pct: util.as_ref().map(|u| u.memory as f32),
        core_mhz: d.clock_info(Clock::Graphics).ok().map(|v| v as f32),
        mem_mhz: d.clock_info(Clock::Memory).ok().map(|v| v as f32),
        power_w: d.power_usage().ok().map(mw),
        power_limit_w: d.enforced_power_limit().ok().map(mw),
        fan_pct: fan.map(|v| v as f32),
        vram_used: mem.as_ref().map(|m| m.used),
        vram_total: mem.as_ref().map(|m| m.total),
        encoder_pct: d.encoder_utilization().ok().map(|u| u.utilization as f32),
        decoder_pct: d.decoder_utilization().ok().map(|u| u.utilization as f32),
        pstate: d.performance_state().ok().and_then(pstate),
        perfcap: d.current_throttle_reasons().map(perfcap).unwrap_or_default(),
    })
}

fn mw(v: u32) -> f32 {
    v as f32 / 1000.0
}

fn pstate(p: PerformanceState) -> Option<String> {
    use PerformanceState::*;
    let n = match p {
        Zero => 0, One => 1, Two => 2, Three => 3, Four => 4, Five => 5, Six => 6, Seven => 7,
        Eight => 8, Nine => 9, Ten => 10, Eleven => 11, Twelve => 12, Thirteen => 13, Fourteen => 14, Fifteen => 15,
        Unknown => return None,
    };
    Some(format!("P{n}"))
}

fn perfcap(r: ThrottleReasons) -> Vec<String> {
    let map = [
        (ThrottleReasons::GPU_IDLE, "Ocioso"),
        (ThrottleReasons::SW_POWER_CAP, "Energia"),
        (ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN, "Freio de energia"),
        (ThrottleReasons::SW_THERMAL_SLOWDOWN, "Térmico"),
        (ThrottleReasons::HW_THERMAL_SLOWDOWN, "Térmico (HW)"),
        (ThrottleReasons::HW_SLOWDOWN, "Hardware"),
        (ThrottleReasons::SYNC_BOOST, "Sync boost"),
        (ThrottleReasons::APPLICATIONS_CLOCKS_SETTING, "Clock fixado"),
        (ThrottleReasons::DISPLAY_CLOCK_SETTING, "Display"),
    ];
    map.iter().filter(|(f, _)| r.contains(*f)).map(|(_, n)| n.to_string()).collect()
}
