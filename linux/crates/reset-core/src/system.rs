//! Informações estáticas do sistema: SO, placa-mãe/BIOS, CPU e memória. No Windows isso vinha do WMI;
//! aqui sai de /etc/os-release, /proc e /sys/class/dmi.

use crate::util::{dmi_clean, read_trim, read_u64};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub os: OsInfo,
    pub board: BoardInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct OsInfo {
    pub name: String,
    pub kernel: String,
    pub hostname: String,
    pub uptime_s: u64,
    pub desktop: Option<String>,
    /// "wayland" ou "x11"
    pub session: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardInfo {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub product: Option<String>,
    pub bios_vendor: Option<String>,
    pub bios_version: Option<String>,
    pub bios_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuInfo {
    pub model: String,
    pub vendor: String,
    pub cores: usize,
    pub threads: usize,
    pub max_mhz: Option<u64>,
    pub base_mhz: Option<u64>,
    pub l2: Option<String>,
    pub l3: Option<String>,
    pub governor: Option<String>,
    /// Extensões relevantes para jogos e benchmarks (AVX2, AVX-512...).
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryInfo {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

pub fn collect() -> SystemInfo {
    SystemInfo { os: os(), board: board(), cpu: cpu(), memory: memory() }
}

fn os() -> OsInfo {
    let release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let name = release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_else(|| "Linux".into());
    let uptime_s = read_trim("/proc/uptime")
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    OsInfo {
        name,
        kernel: read_trim("/proc/sys/kernel/osrelease").unwrap_or_default(),
        hostname: read_trim("/proc/sys/kernel/hostname").unwrap_or_default(),
        uptime_s,
        desktop: std::env::var("XDG_CURRENT_DESKTOP").ok().filter(|s| !s.is_empty()),
        session: std::env::var("XDG_SESSION_TYPE").ok().filter(|s| !s.is_empty()),
    }
}

fn board() -> BoardInfo {
    let dmi = |f: &str| dmi_clean(read_trim(format!("/sys/class/dmi/id/{f}")));
    BoardInfo {
        vendor: dmi("board_vendor"),
        model: dmi("board_name"),
        product: dmi("product_name"),
        bios_vendor: dmi("bios_vendor"),
        bios_version: dmi("bios_version"),
        bios_date: dmi("bios_date"),
    }
}

fn cpu() -> CpuInfo {
    let text = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let field = |k: &str| {
        text.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
    };

    // núcleos físicos = pares (pacote, core id) distintos; threads = blocos "processor"
    let mut threads = 0;
    let mut cores = BTreeSet::new();
    let mut pkg = String::new();
    for l in text.lines() {
        if l.starts_with("processor") {
            threads += 1;
        } else if let Some(v) = l.strip_prefix("physical id") {
            pkg = v.trim_start_matches([' ', '\t', ':']).to_string();
        } else if let Some(v) = l.strip_prefix("core id") {
            cores.insert((pkg.clone(), v.trim_start_matches([' ', '\t', ':']).to_string()));
        }
    }

    let flags: BTreeSet<&str> = text
        .lines()
        .find(|l| l.starts_with("flags"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.split_whitespace().collect())
        .unwrap_or_default();
    let features = [("sse4_2", "SSE4.2"), ("avx", "AVX"), ("avx2", "AVX2"), ("fma", "FMA3"), ("avx512f", "AVX-512"), ("sha_ni", "SHA"), ("aes", "AES-NI"), ("svm", "AMD-V"), ("vmx", "VT-x")]
        .iter()
        .filter(|(f, _)| flags.contains(f))
        .map(|(_, n)| n.to_string())
        .collect();

    let cache = |idx: u32| read_trim(format!("/sys/devices/system/cpu/cpu0/cache/index{idx}/size"));
    let freq = |f: &str| read_u64(format!("/sys/devices/system/cpu/cpu0/cpufreq/{f}")).map(|khz| khz / 1000);

    CpuInfo {
        model: field("model name").unwrap_or_else(|| "CPU desconhecida".into()),
        vendor: field("vendor_id").unwrap_or_default(),
        cores: cores.len().max(1),
        threads: threads.max(1),
        max_mhz: freq("cpuinfo_max_freq"),
        base_mhz: freq("base_frequency"),
        l2: cache(2),
        l3: cache(3),
        governor: read_trim("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        features,
    }
}

pub fn memory() -> MemoryInfo {
    let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.trim_start_matches(':').split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    MemoryInfo { total: kb("MemTotal"), available: kb("MemAvailable"), swap_total: kb("SwapTotal"), swap_free: kb("SwapFree") }
}
