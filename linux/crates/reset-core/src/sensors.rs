//! Sensores em tempo real. No Windows vinham da LibreHardwareMonitorLib (com driver de kernel próprio);
//! no Linux o kernel já expõe tudo em /sys/class/hwmon, então basta ler arquivos. GPU NVIDIA vem do NVML.

use crate::gpu::{self, GpuLive};
use crate::system;
use crate::util::{read_i64, read_trim, read_u64};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Temperature,
    Clock,
    Load,
    Voltage,
    Fan,
    Power,
    Current,
    Data,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensorRow {
    pub id: String,
    /// Peça a que o sensor pertence ("CPU", "SSD Kingston KC3000", "GeForce RTX 4060"...).
    pub hardware: String,
    pub name: String,
    pub kind: Kind,
    pub value: f64,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
}

/// Valores que a visão geral, o overlay e os efeitos de LED usam direto, sem procurar na lista.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub cpu_temp_c: Option<f64>,
    pub cpu_load_pct: Option<f64>,
    /// Núcleo mais ocupado (o gargalo de jogo costuma aparecer aqui antes do total).
    pub cpu_top_core_pct: Option<f64>,
    pub cpu_mhz_avg: Option<f64>,
    pub cpu_mhz_max: Option<f64>,
    pub ram_used: u64,
    pub ram_total: u64,
    pub gpu: Option<GpuLive>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub summary: Summary,
    pub rows: Vec<SensorRow>,
}

#[derive(Default)]
pub struct SensorMonitor {
    prev_cpu: Vec<(u64, u64)>,
    extremes: HashMap<String, (f64, f64)>,
}

impl SensorMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset_extremes(&mut self) {
        self.extremes.clear();
    }

    pub fn poll(&mut self) -> Snapshot {
        let mut rows = Vec::new();
        let mut summary = Summary::default();

        // ---- CPU: carga por núcleo (/proc/stat), clocks (cpufreq) e temperatura (hwmon)
        let cpu_name = "CPU".to_string();
        let loads = self.cpu_loads();
        if let Some((&total, cores)) = loads.split_first() {
            summary.cpu_load_pct = Some(total);
            summary.cpu_top_core_pct = cores.iter().cloned().reduce(f64::max);
            rows.push(raw("cpu/load", &cpu_name, "Carga total", Kind::Load, total, "%"));
            for (i, v) in cores.iter().enumerate() {
                rows.push(raw(&format!("cpu/load/{i}"), &cpu_name, &format!("Thread #{i}"), Kind::Load, *v, "%"));
            }
        }
        let mhz = cpu_clocks();
        if !mhz.is_empty() {
            let avg = mhz.iter().sum::<f64>() / mhz.len() as f64;
            let max = mhz.iter().cloned().fold(0.0, f64::max);
            summary.cpu_mhz_avg = Some(avg);
            summary.cpu_mhz_max = Some(max);
            rows.push(raw("cpu/clock/avg", &cpu_name, "Clock médio", Kind::Clock, avg, "MHz"));
            rows.push(raw("cpu/clock/max", &cpu_name, "Clock máximo", Kind::Clock, max, "MHz"));
        }

        // ---- hwmon: CPU, NVMe, placa-mãe (Super I/O), Wi-Fi, amdgpu...
        // o chip da CPU vem primeiro para as temperaturas ficarem no mesmo grupo da carga e dos clocks
        let mut chips = hwmon_chips();
        chips.sort_by_key(|c| !c.is_cpu);
        for chip in chips {
            for r in &chip.rows {
                if chip.is_cpu && r.kind == Kind::Temperature && summary.cpu_temp_c.is_none() && is_cpu_package(&r.name) {
                    summary.cpu_temp_c = Some(r.value);
                }
            }
            rows.extend(chip.rows);
        }

        // ---- RAM
        let mem = system::memory();
        summary.ram_total = mem.total;
        summary.ram_used = mem.total.saturating_sub(mem.available);
        rows.push(raw("ram/load", "Memória", "Uso", Kind::Load, pct(summary.ram_used, summary.ram_total), "%"));
        rows.push(raw("ram/used", "Memória", "Em uso", Kind::Data, gib(summary.ram_used), "GB"));
        rows.push(raw("ram/free", "Memória", "Disponível", Kind::Data, gib(mem.available), "GB"));

        // ---- GPU NVIDIA
        for (index, h) in gpu::names() {
            let Some(l) = gpu::live(*index) else { continue };
            let k = |s: &str| format!("gpu{index}/{s}");
            let mut add = |id: &str, name: &str, kind, v: Option<f32>, unit| {
                if let Some(v) = v {
                    rows.push(raw(&k(id), h, name, kind, v as f64, unit));
                }
            };
            add("temp", "Núcleo", Kind::Temperature, l.temp_c, "°C");
            add("load", "Núcleo", Kind::Load, l.load_pct, "%");
            add("memctrl", "Controlador de memória", Kind::Load, l.mem_ctrl_pct, "%");
            add("enc", "Encoder (NVENC)", Kind::Load, l.encoder_pct, "%");
            add("dec", "Decoder (NVDEC)", Kind::Load, l.decoder_pct, "%");
            add("clock/core", "Núcleo", Kind::Clock, l.core_mhz, "MHz");
            add("clock/mem", "Memória", Kind::Clock, l.mem_mhz, "MHz");
            add("power", "Consumo", Kind::Power, l.power_w, "W");
            add("fan", "Ventoinha", Kind::Fan, l.fan_pct, "%");
            if let (Some(u), Some(t)) = (l.vram_used, l.vram_total) {
                add("vram", "VRAM em uso", Kind::Data, Some(gib(u) as f32), "GB");
                add("vram/load", "VRAM", Kind::Load, Some(pct(u, t) as f32), "%");
            }
            if summary.gpu.is_none() {
                summary.gpu = Some(l);
            }
        }

        // ---- mínimos e máximos desde a abertura (ou desde o último "zerar")
        for r in &mut rows {
            let e = self.extremes.entry(r.id.clone()).or_insert((r.value, r.value));
            e.0 = e.0.min(r.value);
            e.1 = e.1.max(r.value);
            (r.min, r.max) = *e;
        }
        Snapshot { summary, rows }
    }

    /// [total, thread0, thread1, ...] em % desde a leitura anterior. A primeira chamada compara com o boot.
    fn cpu_loads(&mut self) -> Vec<f64> {
        let text = fs::read_to_string("/proc/stat").unwrap_or_default();
        let now: Vec<(u64, u64)> = text
            .lines()
            .take_while(|l| l.starts_with("cpu"))
            .map(|l| {
                let v: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
                let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0); // idle + iowait
                (v.iter().take(8).sum(), idle)
            })
            .collect();
        let out = now
            .iter()
            .enumerate()
            .map(|(i, &(tot, idle))| {
                let (pt, pi) = self.prev_cpu.get(i).copied().unwrap_or((0, 0));
                let dt = tot.saturating_sub(pt);
                if dt == 0 { 0.0 } else { 100.0 * (1.0 - idle.saturating_sub(pi) as f64 / dt as f64) }
            })
            .collect();
        self.prev_cpu = now;
        out
    }
}

fn raw(id: &str, hw: &str, name: &str, kind: Kind, value: f64, unit: &'static str) -> SensorRow {
    SensorRow { id: id.into(), hardware: hw.into(), name: name.into(), kind, value, unit, min: value, max: value }
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 }
}

fn gib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0 * 1024.0)
}

fn is_cpu_package(label: &str) -> bool {
    ["Tctl", "Tdie", "Package id 0", "CPU"].iter().any(|p| label.starts_with(p))
}

fn cpu_clocks() -> Vec<f64> {
    let Ok(dir) = fs::read_dir("/sys/devices/system/cpu") else { return Vec::new() };
    let mut v: Vec<(usize, f64)> = dir
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().strip_prefix("cpu")?.parse::<usize>().ok()?;
            let khz = read_u64(e.path().join("cpufreq/scaling_cur_freq"))?;
            Some((n, khz as f64 / 1000.0))
        })
        .collect();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

struct Chip {
    is_cpu: bool,
    rows: Vec<SensorRow>,
}

/// Nome amigável do chip hwmon. NVMe ganha o modelo do SSD; drivers de Super I/O viram "Placa-mãe".
fn chip_title(name: &str, dir: &Path) -> (String, bool) {
    match name {
        "k10temp" | "zenpower" | "coretemp" => ("CPU".into(), true),
        "nvme" => {
            let model = read_trim(dir.join("device/model")).unwrap_or_else(|| "NVMe".into());
            (format!("SSD {model}"), false)
        }
        "amdgpu" => ("GPU AMD".into(), false),
        "i915" | "xe" => ("GPU Intel".into(), false),
        "acpitz" => ("ACPI".into(), false),
        n if n.starts_with("nct") || n.starts_with("it87") || n.starts_with("it86") || n.starts_with("asus") || n.starts_with("gigabyte") => {
            (format!("Placa-mãe ({n})"), false)
        }
        n if n.starts_with("mt79") || n.starts_with("iwlwifi") || n.starts_with("ath") => (format!("Wi-Fi ({n})"), false),
        n => (n.to_string(), false),
    }
}

fn hwmon_chips() -> Vec<Chip> {
    let Ok(dir) = fs::read_dir("/sys/class/hwmon") else { return Vec::new() };
    let mut dirs: Vec<_> = dir.flatten().map(|e| e.path()).collect();
    dirs.sort();
    let mut seen: HashMap<String, usize> = HashMap::new();
    dirs.iter()
        .filter_map(|d| {
            let name = read_trim(d.join("name"))?;
            let (mut title, is_cpu) = chip_title(&name, d);
            // dois SSDs do mesmo modelo, por exemplo: numera o segundo
            let n = seen.entry(title.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                title = format!("{title} #{n}");
            }
            let rows = hwmon_rows(d, &title);
            (!rows.is_empty()).then_some(Chip { is_cpu, rows })
        })
        .collect()
}

fn hwmon_rows(dir: &Path, title: &str) -> Vec<SensorRow> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    files.sort_by_key(|f| natural(f));
    let mut rows = Vec::new();
    for f in files {
        // tempN_input, fanN_input, inN_input, powerN_input/average, currN_input, freqN_input
        let Some((base, suffix)) = f.rsplit_once('_') else { continue };
        if suffix != "input" && suffix != "average" {
            continue;
        }
        let prefix: String = base.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let (kind, unit, scale) = match prefix.as_str() {
            "temp" => (Kind::Temperature, "°C", 1e-3),
            "fan" => (Kind::Fan, "RPM", 1.0),
            "in" => (Kind::Voltage, "V", 1e-3),
            "power" => (Kind::Power, "W", 1e-6),
            "curr" => (Kind::Current, "A", 1e-3),
            "freq" => (Kind::Clock, "MHz", 1e-6),
            _ => continue,
        };
        if suffix == "average" && dir.join(format!("{base}_input")).exists() {
            continue; // prefere o valor instantâneo
        }
        let Some(v) = read_i64(dir.join(&f)) else { continue };
        let value = v as f64 * scale;
        // sensores desligados aparecem como 0 RPM / -273 °C / valores absurdos
        if kind == Kind::Temperature && !(-40.0..=200.0).contains(&value) {
            continue;
        }
        let label = read_trim(dir.join(format!("{base}_label"))).unwrap_or_else(|| default_label(&prefix, base));
        rows.push(raw(&format!("{}/{base}", dir.display()), title, &label, kind, value, unit));
    }
    rows
}

fn default_label(prefix: &str, base: &str) -> String {
    let n = base.trim_start_matches(prefix);
    match prefix {
        "temp" => format!("Temperatura {n}"),
        "fan" => format!("Ventoinha {n}"),
        "in" => format!("Tensão {n}"),
        "power" => format!("Consumo {n}"),
        "curr" => format!("Corrente {n}"),
        _ => base.to_string(),
    }
}

/// Chave de ordenação "temp10" depois de "temp9".
fn natural(s: &str) -> (String, u32, String) {
    let p: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &s[p.len()..];
    let d: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (p, d.parse().unwrap_or(0), rest[d.len()..].to_string())
}
