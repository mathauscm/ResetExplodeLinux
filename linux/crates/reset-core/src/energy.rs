//! Energia: bateria, fonte, perfil de energia, política de clock da CPU (governor, EPP, boost) e consumo/limites da GPU.
//! No Windows vinha do WMI (`BatteryInfo.cs`); no Linux o kernel expõe quase tudo em /sys/class/power_supply e
//! /sys/devices/system/cpu/cpufreq. Só leitura aqui; as mudanças são montadas como comando (com valores validados)
//! para a interface rodar com `pkexec`, exceto o perfil de energia, que o power-profiles-daemon troca sem senha.

use crate::gpu;
use crate::util::{read_trim, read_u64};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

const POWER_SUPPLY: &str = "/sys/class/power_supply";
const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";
const RAPL: &str = "/sys/class/powercap/intel-rapl:0";
pub const POWERPROFILESCTL: &str = "/usr/bin/powerprofilesctl";
pub const CPUPOWER: &str = "/usr/bin/cpupower";
const SH: &str = "/usr/bin/sh";

// ---------------------------------------------------------------- bateria

#[derive(Debug, Clone, Serialize)]
pub struct Battery {
    pub name: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub technology: Option<String>,
    /// "Carregando", "Descarregando", "Totalmente carregada"...
    pub status: Option<String>,
    pub charging: bool,
    pub discharging: bool,
    pub capacity_pct: Option<f64>,
    pub design_wh: Option<f64>,
    pub full_wh: Option<f64>,
    pub now_wh: Option<f64>,
    pub power_w: Option<f64>,
    pub voltage_v: Option<f64>,
    pub cycles: Option<u64>,
    pub wear_pct: Option<f64>,
    /// Minutos até descarregar (fora da tomada) ou até encher (carregando).
    pub minutes_left: Option<u64>,
    /// "good" | "warn" | "bad" | "unknown", com o rótulo e a explicação, como os cartões do Windows.
    pub verdict: &'static str,
    pub label: &'static str,
    pub reasons: Vec<String>,
}

/// Bateria de periférico (mouse, fone, controle) que o kernel enxerga: só nome e carga.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceBattery {
    pub name: String,
    pub model: Option<String>,
    pub capacity_pct: Option<f64>,
    /// Alguns só informam faixa: "Full", "High", "Normal", "Low", "Critical".
    pub level: Option<String>,
    pub status: Option<String>,
}

/// Lê um `uevent` do power_supply (`POWER_SUPPLY_CAPACITY=87`) num mapa sem o prefixo.
pub fn parse_uevent(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim_start_matches("POWER_SUPPLY_").to_string(), v.trim().to_string()))
        .collect()
}

fn status_pt(s: &str) -> &'static str {
    match s {
        "Charging" => "Carregando",
        "Discharging" => "Descarregando",
        "Full" => "Totalmente carregada",
        "Not charging" => "Na tomada, sem carregar",
        _ => "Desconhecido",
    }
}

fn tech_pt(s: &str) -> Option<String> {
    let t = match s {
        "Li-ion" => "Íon-lítio",
        "Li-poly" => "Polímero de lítio",
        "NiMH" => "NiMH",
        "NiCd" => "NiCd",
        "LiFe" => "LiFePO4",
        "Unknown" => return None,
        o => o,
    };
    Some(t.to_string())
}

/// Monta a bateria a partir do `uevent`. Os valores do kernel vêm em µWh/µAh/µW/µV; baterias que só
/// informam carga (µAh) viram Wh pela tensão de projeto. O veredito segue o `BatteryInfo.cs`.
pub fn battery_from_uevent(name: &str, m: &HashMap<String, String>, on_ac: bool) -> Battery {
    let f = |k: &str| m.get(k).and_then(|v| v.parse::<f64>().ok());
    let volt_design = f("VOLTAGE_MIN_DESIGN").or(f("VOLTAGE_NOW")).map(|v| v / 1e6);
    let wh = |energy: &str, charge: &str| f(energy).map(|v| v / 1e6).or_else(|| Some(f(charge)? / 1e6 * volt_design?));
    let design = wh("ENERGY_FULL_DESIGN", "CHARGE_FULL_DESIGN").filter(|v| *v > 0.0);
    let full = wh("ENERGY_FULL", "CHARGE_FULL").filter(|v| *v > 0.0);
    let now = wh("ENERGY_NOW", "CHARGE_NOW");
    let voltage = f("VOLTAGE_NOW").map(|v| v / 1e6).filter(|v| *v > 0.0);
    let power = f("POWER_NOW").map(|v| v.abs() / 1e6).or_else(|| Some(f("CURRENT_NOW")?.abs() / 1e6 * voltage?)).filter(|v| *v > 0.0);
    let raw_status = m.get("STATUS").map(String::as_str).unwrap_or("Unknown");
    let charging = raw_status == "Charging";
    let discharging = raw_status == "Discharging";
    let capacity = f("CAPACITY").or_else(|| Some(100.0 * now? / full?)).map(|v| v.clamp(0.0, 100.0));
    let cycles = m.get("CYCLE_COUNT").and_then(|v| v.parse().ok()).filter(|c| *c > 0); // muitos informam 0
    let wear = match (design, full) {
        (Some(d), Some(fu)) => Some((100.0 * (1.0 - fu / d)).max(0.0)),
        _ => None,
    };
    let minutes_left = match (power, now, full) {
        (Some(p), Some(n), _) if discharging => Some((60.0 * n / p) as u64),
        (Some(p), Some(n), Some(fu)) if charging && fu > n => Some((60.0 * (fu - n) / p) as u64),
        _ => None,
    };

    let mut reasons = Vec::new();
    let (mut verdict, mut label) = ("good", "SAUDÁVEL");
    match wear {
        None => {
            (verdict, label) = ("unknown", "SEM DADOS");
            reasons.push("O kernel não informou a capacidade de projeto ou a atual, então o desgaste não pôde ser calculado.".into());
        }
        Some(w) if w >= 50.0 => {
            (verdict, label) = ("bad", "DESGASTADA");
            reasons.push(format!("Perdeu {w:.0}% da capacidade original: a autonomia é metade do que era. Vale trocar a bateria."));
        }
        Some(w) if w >= 30.0 => {
            (verdict, label) = ("warn", "ATENÇÃO");
            reasons.push(format!("Perdeu {w:.0}% da capacidade original. Ainda funciona, mas a autonomia já caiu bastante."));
        }
        Some(w) if w >= 15.0 => reasons.push(format!("Desgaste de {w:.0}%, normal para uma bateria com uso.")),
        Some(w) => reasons.push(format!("Desgaste de apenas {w:.0}%: bateria em ótimo estado.")),
    }
    if let Some(c) = cycles {
        if c >= 1000 && verdict == "good" {
            (verdict, label) = ("warn", "ATENÇÃO");
        }
        if c >= 800 {
            reasons.push(format!("{c} ciclos de carga: perto do fim da vida útil típica das baterias de íon-lítio (500 a 1000)."));
        }
    }
    if capacity.is_some_and(|c| c < 20.0) && !on_ac {
        reasons.push("Carga baixa e fora da tomada.".into());
    }

    Battery {
        name: name.to_string(),
        manufacturer: m.get("MANUFACTURER").filter(|s| !s.is_empty()).cloned(),
        model: m.get("MODEL_NAME").filter(|s| !s.is_empty()).cloned(),
        technology: m.get("TECHNOLOGY").and_then(|t| tech_pt(t)),
        status: Some(status_pt(raw_status).to_string()).filter(|s| s != "Desconhecido"),
        charging,
        discharging,
        capacity_pct: capacity,
        design_wh: design,
        full_wh: full,
        now_wh: now,
        power_w: power,
        voltage_v: voltage,
        cycles,
        wear_pct: wear,
        minutes_left,
        verdict,
        label,
        reasons,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerSource {
    pub batteries: Vec<Battery>,
    pub devices: Vec<DeviceBattery>,
    /// Some(true) na tomada; None quando não há adaptador declarado (PC de mesa).
    pub ac_online: Option<bool>,
    /// "Tomada (PC de mesa)", "Tomada", "Bateria".
    pub source: String,
}

pub fn power_source() -> PowerSource {
    let mut batteries = Vec::new();
    let mut devices = Vec::new();
    let mut ac: Option<bool> = None;
    let mut entries: Vec<_> = fs::read_dir(POWER_SUPPLY).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    entries.sort();
    let mut raw = Vec::new();
    for p in &entries {
        let m = parse_uevent(&fs::read_to_string(p.join("uevent")).unwrap_or_default());
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if m.get("TYPE").is_some_and(|t| t == "Mains" || t == "USB") && m.get("ONLINE").is_some_and(|v| v == "1") {
            ac = Some(true);
        } else if m.get("TYPE").is_some_and(|t| t == "Mains") {
            ac.get_or_insert(false);
        }
        raw.push((name, m));
    }
    for (name, m) in &raw {
        if m.get("TYPE").map(String::as_str) != Some("Battery") {
            continue;
        }
        // SCOPE=Device: bateria de periférico (mouse, fone), não alimenta o PC
        if m.get("SCOPE").is_some_and(|s| s == "Device") {
            devices.push(DeviceBattery {
                name: name.clone(),
                model: m.get("MODEL_NAME").cloned(),
                capacity_pct: m.get("CAPACITY").and_then(|v| v.parse().ok()),
                level: m.get("CAPACITY_LEVEL").cloned().filter(|l| l != "Unknown"),
                status: m.get("STATUS").map(|s| status_pt(s).to_string()).filter(|s| s != "Desconhecido"),
            });
            continue;
        }
        batteries.push(battery_from_uevent(name, m, ac == Some(true)));
    }
    // sem adaptador declarado mas a bateria carregando: está na tomada
    if ac.is_none() && batteries.iter().any(|b| b.charging || b.status.as_deref() == Some("Totalmente carregada")) {
        ac = Some(true);
    }
    let source = if batteries.is_empty() {
        "Tomada (PC de mesa)"
    } else if ac == Some(false) || (ac.is_none() && batteries.iter().any(|b| b.discharging)) {
        "Bateria"
    } else {
        "Tomada"
    };
    PowerSource { batteries, devices, ac_online: ac, source: source.into() }
}

// ---------------------------------------------------------------- perfil de energia (power-profiles-daemon)

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PowerProfile {
    /// "performance" | "balanced" | "power-saver"
    pub id: String,
    pub label: String,
    pub active: bool,
    pub cpu_driver: Option<String>,
    pub platform_driver: Option<String>,
    /// Motivo de o perfil de desempenho estar rebaixado ("lap-detected", "high-operating-temperature").
    pub degraded: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Profiles {
    /// false: power-profiles-daemon não está instalado ou não responde.
    pub available: bool,
    pub profiles: Vec<PowerProfile>,
    pub active: Option<String>,
    pub note: Option<String>,
}

fn profile_label(id: &str) -> String {
    match id {
        "performance" => "Desempenho",
        "balanced" => "Equilibrado",
        "power-saver" => "Economia",
        o => o,
    }
    .to_string()
}

/// Lê a saída do `powerprofilesctl list`:
/// ```text
/// * performance:
///     CpuDriver:  amd_pstate
///     Degraded:   no
///
///   balanced:
///     CpuDriver:  amd_pstate
/// ```
pub fn parse_profiles(text: &str) -> Vec<PowerProfile> {
    let mut out: Vec<PowerProfile> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let indented = line.starts_with("    ") || line.starts_with('\t');
        if !indented && t.ends_with(':') {
            let active = t.starts_with('*');
            let id = t.trim_start_matches('*').trim().trim_end_matches(':').to_string();
            out.push(PowerProfile { label: profile_label(&id), id, active, cpu_driver: None, platform_driver: None, degraded: None });
        } else if let (Some(p), Some((k, v))) = (out.last_mut(), t.split_once(':')) {
            let v = v.trim().to_string();
            match k.trim() {
                "CpuDriver" => p.cpu_driver = Some(v),
                "PlatformDriver" if v != "placeholder" => p.platform_driver = Some(v),
                "Degraded" if v != "no" => p.degraded = Some(v),
                _ => {}
            }
        }
    }
    out
}

pub fn profiles() -> Profiles {
    if !Path::new(POWERPROFILESCTL).exists() {
        return Profiles {
            available: false,
            profiles: Vec::new(),
            active: None,
            note: Some("O power-profiles-daemon não está instalado. Com ele, o GNOME e o KDE trocam entre Desempenho, Equilibrado e Economia.".into()),
        };
    }
    let out = Command::new(POWERPROFILESCTL).arg("list").env("LC_ALL", "C").output();
    match out {
        Ok(o) if o.status.success() => {
            let profiles = parse_profiles(&String::from_utf8_lossy(&o.stdout));
            let active = profiles.iter().find(|p| p.active).map(|p| p.id.clone());
            Profiles { available: !profiles.is_empty(), profiles, active, note: None }
        }
        _ => Profiles {
            available: false,
            profiles: Vec::new(),
            active: None,
            note: Some("O power-profiles-daemon está instalado mas não respondeu. Ative o serviço: systemctl enable --now power-profiles-daemon.".into()),
        },
    }
}

/// Troca o perfil (não precisa de root: o daemon autoriza a sessão ativa pelo polkit).
pub fn set_profile(id: &str) -> Result<(), String> {
    let p = profiles();
    if !p.profiles.iter().any(|x| x.id == id) {
        return Err(format!("perfil desconhecido: {id}"));
    }
    let o = Command::new(POWERPROFILESCTL).args(["set", id]).env("LC_ALL", "C").output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().lines().last().unwrap_or("o power-profiles-daemon recusou a troca").to_string())
    }
}

// ---------------------------------------------------------------- CPU: driver, governor, EPP, boost

#[derive(Debug, Clone, Serialize)]
pub struct CpuPolicy {
    pub id: u32,
    pub cpus: String,
    pub governor: Option<String>,
    pub epp: Option<String>,
    pub cur_mhz: Option<f64>,
    pub min_mhz: Option<f64>,
    pub max_mhz: Option<f64>,
    /// Ranking de núcleo preferido do amd-pstate (maior = núcleo mais rápido, recebe as tarefas pesadas).
    pub prefcore_rank: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuPower {
    /// "amd-pstate-epp", "intel_pstate", "acpi-cpufreq"...
    pub driver: Option<String>,
    /// amd_pstate / intel_pstate: "active", "passive", "guided".
    pub driver_mode: Option<String>,
    pub governors: Vec<String>,
    pub epps: Vec<String>,
    /// Governor/EPP comuns a todos os núcleos; None quando estão diferentes ("misto").
    pub governor: Option<String>,
    pub epp: Option<String>,
    pub boost: Option<bool>,
    pub prefcore: Option<bool>,
    pub hw_min_mhz: Option<f64>,
    pub hw_max_mhz: Option<f64>,
    pub policies: Vec<CpuPolicy>,
    /// Com o amd-pstate ativo e o governor "performance", o EPP fica preso em "performance".
    pub epp_locked: bool,
    pub can_change: bool,
}

fn khz(p: &Path, f: &str) -> Option<f64> {
    read_u64(p.join(f)).map(|v| v as f64 / 1000.0)
}

fn words(p: impl AsRef<Path>) -> Vec<String> {
    read_trim(p).map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_default()
}

fn policy_dirs() -> Vec<(u32, std::path::PathBuf)> {
    let mut v: Vec<_> = fs::read_dir(CPUFREQ)
        .map(|d| {
            d.flatten()
                .filter_map(|e| Some((e.file_name().to_string_lossy().strip_prefix("policy")?.parse::<u32>().ok()?, e.path())))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|(i, _)| *i);
    v
}

/// O mesmo valor em todos, ou None.
fn common<T: PartialEq + Clone>(it: impl IntoIterator<Item = Option<T>>) -> Option<T> {
    let mut it = it.into_iter();
    let first = it.next()??;
    it.all(|x| x.as_ref() == Some(&first)).then_some(first)
}

pub fn cpu_power() -> Option<CpuPower> {
    let dirs = policy_dirs();
    let (_, p0) = dirs.first()?;
    let policies: Vec<CpuPolicy> = dirs
        .iter()
        .map(|(id, p)| CpuPolicy {
            id: *id,
            cpus: read_trim(p.join("affected_cpus")).unwrap_or_default(),
            governor: read_trim(p.join("scaling_governor")),
            epp: read_trim(p.join("energy_performance_preference")),
            cur_mhz: khz(p, "scaling_cur_freq"),
            min_mhz: khz(p, "scaling_min_freq"),
            max_mhz: khz(p, "scaling_max_freq"),
            prefcore_rank: read_u64(p.join("amd_pstate_prefcore_ranking")).map(|v| v as u32),
        })
        .collect();
    let driver = read_trim(p0.join("scaling_driver"));
    let driver_mode = read_trim("/sys/devices/system/cpu/amd_pstate/status").or_else(|| read_trim("/sys/devices/system/cpu/intel_pstate/status"));
    // boost: global no acpi-cpufreq e no amd-pstate; no intel_pstate é o inverso de no_turbo
    let boost = read_u64(format!("{CPUFREQ}/boost"))
        .map(|v| v == 1)
        .or_else(|| read_u64("/sys/devices/system/cpu/intel_pstate/no_turbo").map(|v| v == 0))
        .or_else(|| common(dirs.iter().map(|(_, p)| read_u64(p.join("boost")))).map(|v| v == 1));
    let governor = common(policies.iter().map(|p| p.governor.clone()));
    let epp = common(policies.iter().map(|p| p.epp.clone()));
    let epp_locked = driver.as_deref() == Some("amd-pstate-epp") && governor.as_deref() == Some("performance");
    Some(CpuPower {
        governors: words(p0.join("scaling_available_governors")),
        epps: words(p0.join("energy_performance_available_preferences")),
        hw_min_mhz: khz(p0, "cpuinfo_min_freq"),
        hw_max_mhz: khz(p0, "cpuinfo_max_freq"),
        prefcore: read_u64("/sys/devices/system/cpu/amd_pstate/prefcore").map(|v| v == 1).or_else(|| read_trim("/sys/devices/system/cpu/amd_pstate/prefcore").map(|s| s == "enabled")),
        driver,
        driver_mode,
        governor,
        epp,
        boost,
        policies,
        epp_locked,
        can_change: Path::new(CPUPOWER).exists() || Path::new(SH).exists(),
    })
}

/// Clock atual de cada política (MHz), para o gráfico ao vivo.
pub fn cpu_clocks() -> Vec<f64> {
    policy_dirs().iter().filter_map(|(_, p)| khz(p, "scaling_cur_freq")).collect()
}

/// O que dá para mudar na CPU. Sempre com root (pkexec).
#[derive(Debug, Clone, PartialEq)]
pub enum CpuChange {
    Governor(String),
    Epp(String),
    Boost(bool),
}

fn token_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Monta o comando (programa, argumentos) da mudança. O valor tem que estar na lista que o próprio kernel
/// oferece (`available`) e ser só letras/números/`_`/`-`: nada de texto livre chega ao shell.
/// Usa o `cpupower` quando existe; senão, `sh -c` escrevendo direto no sysfs.
pub fn cpu_change_command(change: &CpuChange, available: &[String], has_cpupower: bool) -> Result<(String, Vec<String>), String> {
    let check = |v: &str, what: &str| {
        if token_ok(v) && available.iter().any(|a| a == v) {
            Ok(())
        } else {
            Err(format!("{what} não suportado por esta CPU: {v}"))
        }
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match change {
        CpuChange::Governor(g) => {
            check(g, "governor")?;
            Ok(if has_cpupower {
                (CPUPOWER.into(), s(&["frequency-set", "-g", g]))
            } else {
                (SH.into(), vec!["-c".into(), format!("for f in {CPUFREQ}/policy*/scaling_governor; do echo {g} > \"$f\"; done")])
            })
        }
        CpuChange::Epp(e) => {
            check(e, "preferência de energia (EPP)")?;
            Ok(if has_cpupower {
                (CPUPOWER.into(), s(&["set", "--epp", e]))
            } else {
                (SH.into(), vec!["-c".into(), format!("for f in {CPUFREQ}/policy*/energy_performance_preference; do echo {e} > \"$f\"; done")])
            })
        }
        CpuChange::Boost(on) => {
            let v = if *on { "1" } else { "0" };
            Ok(if has_cpupower {
                (CPUPOWER.into(), s(&["set", "--turbo-boost", v]))
            } else {
                (SH.into(), vec!["-c".into(), format!("echo {v} > {CPUFREQ}/boost")])
            })
        }
    }
}

/// Comando da mudança, validado contra o que a CPU deste PC oferece agora.
pub fn cpu_change(change: &CpuChange) -> Result<(String, Vec<String>), String> {
    let cpu = cpu_power().ok_or("esta CPU não expõe o cpufreq")?;
    let available = match change {
        CpuChange::Governor(_) => cpu.governors,
        CpuChange::Epp(_) => {
            if cpu.epp_locked {
                return Err("com o governor \"performance\" o amd-pstate trava o EPP em \"performance\". Troque o governor para \"powersave\" antes.".into());
            }
            cpu.epps
        }
        CpuChange::Boost(_) => {
            if cpu.boost.is_none() {
                return Err("esta CPU não permite ligar/desligar o boost por aqui".into());
            }
            Vec::new()
        }
    };
    cpu_change_command(change, &available, Path::new(CPUPOWER).exists())
}

// ---------------------------------------------------------------- consumo: GPU (NVML) e CPU (RAPL)

#[derive(Debug, Clone, Serialize)]
pub struct GpuPower {
    pub index: u32,
    pub name: String,
    pub power_w: Option<f32>,
    /// "sensor" (leitura direta) ou "amostras" (média das amostras do driver, quando a placa não dá a leitura direta).
    pub power_from: Option<&'static str>,
    pub limit_w: Option<f32>,
    pub default_w: Option<f32>,
    pub min_w: Option<f32>,
    pub max_w: Option<f32>,
    pub pstate: Option<String>,
    pub perfcap: Vec<String>,
    pub load_pct: Option<f32>,
}

pub fn gpu_power() -> Vec<GpuPower> {
    gpu::list()
        .into_iter()
        .map(|g| {
            let l = gpu::live(g.index).unwrap_or_default();
            let power_w = l.power_w;
            let power_from = power_w.map(|_| if l.power_sampled { "amostras" } else { "sensor" });
            GpuPower {
                index: g.index,
                name: g.name,
                power_w,
                power_from,
                limit_w: l.power_limit_w,
                default_w: g.power_default_w,
                min_w: g.power_min_w,
                max_w: g.power_max_w,
                pstate: l.pstate,
                perfcap: l.perfcap,
                load_pct: l.load_pct,
            }
        })
        .collect()
}

/// Medidor de energia da CPU (RAPL): o consumo sai da diferença entre duas leituras.
#[derive(Default)]
pub struct CpuEnergyMeter {
    prev: Option<(Instant, u64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuEnergy {
    pub available: bool,
    pub power_w: Option<f64>,
    /// Por que não dá para ler: o kernel só libera o contador para root desde 2020 (CVE-2020-8694).
    pub note: Option<String>,
}

impl CpuEnergyMeter {
    pub fn poll(&mut self) -> CpuEnergy {
        let path = Path::new(RAPL).join("energy_uj");
        if !path.exists() {
            return CpuEnergy { available: false, power_w: None, note: Some("Esta CPU não expõe o medidor de energia (RAPL).".into()) };
        }
        let Some(uj) = read_u64(&path) else {
            return CpuEnergy {
                available: false,
                power_w: None,
                note: Some("O kernel só libera o medidor de energia da CPU (RAPL) para o administrador.".into()),
            };
        };
        let max = read_u64(Path::new(RAPL).join("max_energy_range_uj")).unwrap_or(u64::MAX);
        let now = Instant::now();
        let w = self.prev.and_then(|(t, e)| rapl_watts(e, uj, max, now.duration_since(t).as_secs_f64()));
        self.prev = Some((now, uj));
        CpuEnergy { available: true, power_w: w, note: None }
    }
}

/// Watts entre duas leituras do contador (µJ), tratando a volta do contador ao zero.
pub fn rapl_watts(prev: u64, now: u64, max_range: u64, secs: f64) -> Option<f64> {
    if secs < 0.2 {
        return None;
    }
    let delta = if now >= prev { now - prev } else { max_range.saturating_sub(prev) + now };
    Some(delta as f64 / 1e6 / secs)
}

// ---------------------------------------------------------------- tudo junto

#[derive(Debug, Clone, Serialize)]
pub struct EnergyInfo {
    pub source: PowerSource,
    pub profiles: Profiles,
    pub cpu: Option<CpuPower>,
    pub gpus: Vec<GpuPower>,
}

pub fn overview() -> EnergyInfo {
    EnergyInfo { source: power_source(), profiles: profiles(), cpu: cpu_power(), gpus: gpu_power() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: &str = "POWER_SUPPLY_NAME=BAT0
POWER_SUPPLY_TYPE=Battery
POWER_SUPPLY_STATUS=Discharging
POWER_SUPPLY_PRESENT=1
POWER_SUPPLY_TECHNOLOGY=Li-ion
POWER_SUPPLY_CYCLE_COUNT=412
POWER_SUPPLY_VOLTAGE_MIN_DESIGN=11550000
POWER_SUPPLY_VOLTAGE_NOW=12100000
POWER_SUPPLY_POWER_NOW=9500000
POWER_SUPPLY_ENERGY_FULL_DESIGN=57000000
POWER_SUPPLY_ENERGY_FULL=45600000
POWER_SUPPLY_ENERGY_NOW=22800000
POWER_SUPPLY_CAPACITY=50
POWER_SUPPLY_MODEL_NAME=5B10W13930
POWER_SUPPLY_MANUFACTURER=SMP
";

    #[test]
    fn bateria_por_energia() {
        let b = battery_from_uevent("BAT0", &parse_uevent(LAPTOP), false);
        assert_eq!(b.design_wh, Some(57.0));
        assert_eq!(b.full_wh, Some(45.6));
        assert!((b.wear_pct.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(b.verdict, "good");
        assert_eq!(b.status.as_deref(), Some("Descarregando"));
        assert_eq!(b.technology.as_deref(), Some("Íon-lítio"));
        assert_eq!(b.cycles, Some(412));
        assert_eq!(b.power_w, Some(9.5));
        // 22,8 Wh a 9,5 W = 2,4 h
        assert_eq!(b.minutes_left, Some(144));
    }

    #[test]
    fn bateria_por_carga_e_desgastada() {
        let t = "POWER_SUPPLY_TYPE=Battery\nPOWER_SUPPLY_STATUS=Charging\nPOWER_SUPPLY_VOLTAGE_MIN_DESIGN=7600000\n\
                 POWER_SUPPLY_VOLTAGE_NOW=8000000\nPOWER_SUPPLY_CURRENT_NOW=2000000\nPOWER_SUPPLY_CHARGE_FULL_DESIGN=5000000\n\
                 POWER_SUPPLY_CHARGE_FULL=2000000\nPOWER_SUPPLY_CHARGE_NOW=1000000\nPOWER_SUPPLY_CYCLE_COUNT=0\n";
        let b = battery_from_uevent("BAT1", &parse_uevent(t), true);
        assert!((b.design_wh.unwrap() - 38.0).abs() < 1e-9);
        assert!((b.wear_pct.unwrap() - 60.0).abs() < 1e-9);
        assert_eq!((b.verdict, b.label), ("bad", "DESGASTADA"));
        assert_eq!(b.cycles, None, "0 ciclos = não informado");
        assert_eq!(b.capacity_pct, Some(50.0), "sem CAPACITY, calcula pela carga");
        assert_eq!(b.power_w, Some(16.0));
        assert!(b.charging && b.minutes_left.is_some());
    }

    #[test]
    fn bateria_sem_dados() {
        let b = battery_from_uevent("BAT0", &parse_uevent("POWER_SUPPLY_TYPE=Battery\nPOWER_SUPPLY_CAPACITY=10\n"), false);
        assert_eq!(b.verdict, "unknown");
        assert!(b.reasons.iter().any(|r| r.contains("Carga baixa")));
    }

    #[test]
    fn perfis() {
        let t = "* performance:\n    CpuDriver:\tamd_pstate\n    Degraded:   no\n\n  balanced:\n    CpuDriver:\tamd_pstate\n    PlatformDriver:\tplaceholder\n\n  power-saver:\n    CpuDriver:\tamd_pstate\n    PlatformDriver:\tplatform_profile\n";
        let p = parse_profiles(t);
        assert_eq!(p.len(), 3);
        assert!(p[0].active && !p[1].active);
        assert_eq!(p[0].id, "performance");
        assert_eq!(p[0].label, "Desempenho");
        assert_eq!(p[0].cpu_driver.as_deref(), Some("amd_pstate"));
        assert_eq!(p[0].degraded, None);
        assert_eq!(p[1].platform_driver, None, "placeholder não conta");
        assert_eq!(p[2].platform_driver.as_deref(), Some("platform_profile"));
        let d = parse_profiles("  performance:\n    Degraded:   high-operating-temperature\n* balanced:\n");
        assert_eq!(d[0].degraded.as_deref(), Some("high-operating-temperature"));
        assert!(d[1].active);
    }

    #[test]
    fn comando_da_cpu_so_com_valor_valido() {
        let govs = vec!["performance".to_string(), "powersave".to_string()];
        let (prog, args) = cpu_change_command(&CpuChange::Governor("powersave".into()), &govs, true).unwrap();
        assert_eq!(prog, CPUPOWER);
        assert_eq!(args, ["frequency-set", "-g", "powersave"]);
        // fora da lista, ou com caracteres de shell: recusado
        assert!(cpu_change_command(&CpuChange::Governor("ondemand".into()), &govs, true).is_err());
        let evil = vec!["x; rm -rf /".to_string()];
        assert!(cpu_change_command(&CpuChange::Governor("x; rm -rf /".into()), &evil, false).is_err());
        let (prog, args) = cpu_change_command(&CpuChange::Epp("balance_power".into()), &["balance_power".into()], false).unwrap();
        assert_eq!(prog, SH);
        assert!(args[1].contains("echo balance_power > \"$f\""));
        let (_, args) = cpu_change_command(&CpuChange::Boost(false), &[], true).unwrap();
        assert_eq!(args, ["set", "--turbo-boost", "0"]);
    }

    #[test]
    fn rapl() {
        assert_eq!(rapl_watts(1_000_000, 51_000_000, u64::MAX, 1.0), Some(50.0));
        // o contador voltou ao zero
        assert_eq!(rapl_watts(99_000_000, 1_000_000, 100_000_000, 0.5), Some(4.0));
        assert_eq!(rapl_watts(0, 1, 10, 0.01), None);
    }

    #[test]
    fn valor_comum() {
        assert_eq!(common([Some(1), Some(1)]), Some(1));
        assert_eq!(common([Some(1), Some(2)]), None);
        assert_eq!(common::<u8>([]), None);
    }
}
