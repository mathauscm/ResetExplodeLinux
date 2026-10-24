//! Overclock. GPU NVIDIA pelo NVML (o mesmo caminho do `MainWindow.Oc.cs`): limite de energia, offset de clock do
//! núcleo e da memória e teto de clock. Ler não precisa de nada; mudar exige root, então o app roda o próprio
//! executável com `pkexec` no modo `--nvml-apply` (ver `cli_main`), que valida tudo de novo antes de aplicar.
//! Tudo volta ao padrão ao reiniciar o PC (ou recarregar o driver).
//!
//! Também ficam aqui: o que dá para ver da CPU (amd-pstate, governor, boost; PBO e Curve Optimizer são na BIOS),
//! os erros de hardware do log do kernel (Xid da NVIDIA, MCE da CPU) e o verificador de estabilidade (o `OcCheck`
//! do Windows): carga curta + observação de clocks, temperatura e limitadores.

use crate::gpu;
use crate::sensors::SensorMonitor;
use crate::util::{read_trim, read_u64};
use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState, TemperatureThreshold};
use nvml_wrapper::enums::device::GpuLockedClocksSetting;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::{Device, Nvml};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Faixa que o app aceita para os offsets, além da faixa que o próprio driver informa (que vai a ±1000 MHz no
/// núcleo e +6000 na memória, muito além do que alguma placa aguenta). Negativo é sempre seguro.
pub const CORE_OFFSET_SAFE: (i32, i32) = (-500, 300);
pub const MEM_OFFSET_SAFE: (i32, i32) = (-1000, 1500);

/// Temperaturas em que o verificador para (as mesmas do app do Windows).
pub const CPU_LIMIT_C: f64 = 95.0;
pub const GPU_LIMIT_C: f32 = 88.0;

static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

fn nvml() -> Option<&'static Nvml> {
    NVML.get_or_init(|| Nvml::init().ok()).as_ref()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OffsetInfo {
    pub current: i32,
    /// Faixa informada pelo driver.
    pub min: i32,
    pub max: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuOc {
    pub index: u32,
    pub name: String,
    pub live: gpu::GpuLive,
    pub max_core_mhz: Option<u32>,
    pub max_mem_mhz: Option<u32>,
    pub power_default_w: Option<f32>,
    pub power_min_w: Option<f32>,
    pub power_max_w: Option<f32>,
    /// Limite configurado agora (pode ser diferente do aplicado, `live.power_limit_w`, em placas com mais de um limite).
    pub power_limit_w: Option<f32>,
    pub core_offset: Option<OffsetInfo>,
    pub mem_offset: Option<OffsetInfo>,
    /// Temperatura em que o driver começa a reduzir o clock e em que desliga.
    pub slowdown_c: Option<u32>,
    pub shutdown_c: Option<u32>,
    pub limits: Limits,
}

/// O que dá para pedir a esta placa: a faixa do driver cruzada com a faixa segura do app.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Limits {
    pub power_min_w: f32,
    pub power_max_w: f32,
    pub power_default_w: f32,
    /// None: o driver não deixa mexer no offset.
    pub core: Option<(i32, i32)>,
    pub mem: Option<(i32, i32)>,
    pub cap_min: u32,
    pub cap_max: u32,
}

impl Limits {
    pub fn new(power: (f32, f32, f32), core: Option<OffsetInfo>, mem: Option<OffsetInfo>, max_core_mhz: u32) -> Self {
        let clamp = |o: Option<OffsetInfo>, safe: (i32, i32)| {
            o.map(|o| (o.min.max(safe.0), o.max.min(safe.1))).filter(|(a, b)| a <= b)
        };
        Limits {
            power_min_w: power.0,
            power_max_w: power.1,
            power_default_w: power.2,
            core: clamp(core, CORE_OFFSET_SAFE),
            mem: clamp(mem, MEM_OFFSET_SAFE),
            cap_min: max_core_mhz / 2,
            cap_max: max_core_mhz,
        }
    }
}

fn offset(d: &Device, clock: Clock) -> Option<OffsetInfo> {
    // API por P-state (driver 555+), a mesma que o LACT usa. Vale para o P0, o estado dos jogos. A API antiga
    // (VF offset) não informa a faixa permitida, então sem esta o app não deixa mexer.
    d.clock_offset(clock, PerformanceState::Zero)
        .ok()
        .map(|o| OffsetInfo { current: o.clock_offset_mhz, min: o.min_clock_offset_mhz, max: o.max_clock_offset_mhz })
}

fn read_gpu(nv: &Nvml, index: u32) -> Option<GpuOc> {
    let d = nv.device_by_index(index).ok()?;
    let mw = |v: u32| v as f32 / 1000.0;
    let pm = d.power_management_limit_constraints().ok();
    let default = d.power_management_limit_default().ok().map(mw);
    let core_offset = offset(&d, Clock::Graphics);
    let mem_offset = offset(&d, Clock::Memory);
    let max_core = d.max_clock_info(Clock::Graphics).ok();
    let power = match (pm.as_ref(), default) {
        (Some(p), Some(def)) => (mw(p.min_limit), mw(p.max_limit), def),
        // sem faixa informada: só o padrão vale
        (_, Some(def)) => (def, def, def),
        _ => (0.0, 0.0, 0.0),
    };
    Some(GpuOc {
        index,
        name: d.name().unwrap_or_else(|_| "NVIDIA".into()),
        live: gpu::live(index).unwrap_or_default(),
        max_core_mhz: max_core,
        max_mem_mhz: d.max_clock_info(Clock::Memory).ok(),
        power_default_w: default,
        power_min_w: pm.as_ref().map(|p| mw(p.min_limit)),
        power_max_w: pm.as_ref().map(|p| mw(p.max_limit)),
        power_limit_w: d.power_management_limit().ok().map(mw),
        core_offset,
        mem_offset,
        slowdown_c: d.temperature_threshold(TemperatureThreshold::Slowdown).ok(),
        shutdown_c: d.temperature_threshold(TemperatureThreshold::Shutdown).ok(),
        limits: Limits::new(power, core_offset, mem_offset, max_core.unwrap_or(0)),
    })
}

/// Placas NVIDIA com o que dá para ajustar. Vazio sem driver NVIDIA.
pub fn gpus() -> Vec<GpuOc> {
    let Some(nv) = nvml() else { return Vec::new() };
    (0..nv.device_count().unwrap_or(0)).filter_map(|i| read_gpu(nv, i)).collect()
}

// ------------------------------------------------------------------ pedido, validação e linha de comando

/// O que a tela pede. Sempre o estado completo: o que não mudou vai com o valor atual.
#[derive(Debug, Clone, Deserialize)]
pub struct OcRequest {
    pub index: u32,
    pub power_w: f32,
    pub core_offset: i32,
    pub mem_offset: i32,
    /// Teto de clock do núcleo em MHz; None remove o teto.
    pub clock_cap: Option<u32>,
}

/// Pedido já validado, pronto para virar argumentos do `--nvml-apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub index: u32,
    pub power_mw: u32,
    pub core_offset: i32,
    pub mem_offset: i32,
    pub clock_cap: Option<u32>,
}

pub fn validate(r: &OcRequest, l: &Limits) -> Result<Plan, String> {
    if !r.power_w.is_finite() || l.power_max_w <= 0.0 {
        return Err("esta placa não informa os limites de energia.".into());
    }
    // meio watt de folga para o arredondamento do controle deslizante
    if r.power_w < l.power_min_w - 0.5 || r.power_w > l.power_max_w + 0.5 {
        return Err(format!("limite de energia fora da faixa da placa ({:.0} a {:.0} W).", l.power_min_w, l.power_max_w));
    }
    let power_mw = ((r.power_w * 1000.0).round() as u32).clamp((l.power_min_w * 1000.0) as u32, (l.power_max_w * 1000.0) as u32);
    let check = |v: i32, range: Option<(i32, i32)>, what: &str| match range {
        _ if v == 0 => Ok(()),
        None => Err(format!("o driver não permite mudar o offset {what} nesta placa.")),
        Some((a, b)) if v < a || v > b => Err(format!("offset {what} fora da faixa permitida ({a:+} a {b:+} MHz).")),
        Some(_) => Ok(()),
    };
    check(r.core_offset, l.core, "do núcleo")?;
    check(r.mem_offset, l.mem, "da memória")?;
    if let Some(c) = r.clock_cap {
        if c < l.cap_min || c > l.cap_max {
            return Err(format!("teto de clock fora da faixa ({} a {} MHz).", l.cap_min, l.cap_max));
        }
    }
    Ok(Plan { index: r.index, power_mw, core_offset: r.core_offset, mem_offset: r.mem_offset, clock_cap: r.clock_cap })
}

/// Pedido que devolve a placa ao padrão de fábrica.
pub fn reset_request(index: u32, l: &Limits) -> OcRequest {
    OcRequest { index, power_w: l.power_default_w, core_offset: 0, mem_offset: 0, clock_cap: None }
}

pub const CLI_FLAG: &str = "--nvml-apply";

impl Plan {
    /// Argumentos para `pkexec <exe> ...`.
    pub fn args(&self) -> Vec<String> {
        vec![
            CLI_FLAG.into(),
            "--gpu".into(),
            self.index.to_string(),
            "--power-mw".into(),
            self.power_mw.to_string(),
            "--core-offset".into(),
            self.core_offset.to_string(),
            "--mem-offset".into(),
            self.mem_offset.to_string(),
            "--clock-cap".into(),
            self.clock_cap.map_or("none".into(), |c| c.to_string()),
        ]
    }

    /// Lê de volta os argumentos (tudo depois de `--nvml-apply`). Todos são obrigatórios.
    pub fn parse(args: &[String]) -> Result<Plan, String> {
        let get = |name: &str| -> Result<&str, String> {
            let i = args.iter().position(|a| a == name).ok_or(format!("faltou {name}"))?;
            args.get(i + 1).map(|s| s.as_str()).ok_or(format!("faltou o valor de {name}"))
        };
        let num = |name: &str| get(name).and_then(|v| v.parse::<i64>().map_err(|_| format!("valor inválido em {name}: {v}")));
        let u = |name: &str| num(name).and_then(|v| u32::try_from(v).map_err(|_| format!("valor inválido em {name}")));
        let i = |name: &str| num(name).and_then(|v| i32::try_from(v).map_err(|_| format!("valor inválido em {name}")));
        let cap = match get("--clock-cap")? {
            "none" => None,
            v => Some(v.parse::<u32>().map_err(|_| format!("valor inválido em --clock-cap: {v}"))?),
        };
        Ok(Plan { index: u("--gpu")?, power_mw: u("--power-mw")?, core_offset: i("--core-offset")?, mem_offset: i("--mem-offset")?, clock_cap: cap })
    }
}

fn explain(e: &NvmlError) -> String {
    match e {
        NvmlError::NoPermission => "sem permissão (precisa de administrador)".into(),
        NvmlError::NotSupported => "esta placa ou driver não permite".into(),
        NvmlError::InvalidArg => "valor recusado pelo driver".into(),
        NvmlError::FailedToLoadSymbol(_) => "o driver instalado não tem esta função (atualize o driver)".into(),
        e => e.to_string(),
    }
}

/// Modo `--nvml-apply` (rodado com root via pkexec): valida de novo contra os limites lidos agora e aplica.
/// Escreve uma linha por ajuste (a tela mostra ao vivo) e devolve o código de saída.
pub fn cli_main(args: &[String]) -> i32 {
    let plan = match Plan::parse(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Pedido inválido: {e}");
            return 2;
        }
    };
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("Precisa rodar como administrador.");
        return 2;
    }
    let nv = match Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("NVML indisponível: {e}");
            return 1;
        }
    };
    let Some(info) = read_gpu(&nv, plan.index) else {
        eprintln!("GPU {} não encontrada.", plan.index);
        return 1;
    };
    let req = OcRequest {
        index: plan.index,
        power_w: plan.power_mw as f32 / 1000.0,
        core_offset: plan.core_offset,
        mem_offset: plan.mem_offset,
        clock_cap: plan.clock_cap,
    };
    if let Err(e) = validate(&req, &info.limits) {
        eprintln!("Recusado: {e}");
        return 2;
    }
    let Ok(mut d) = nv.device_by_index(plan.index) else { return 1 };
    println!("{}", info.name);
    let mut ok = true;
    let mut report = |what: String, r: Result<(), NvmlError>| match r {
        Ok(()) => println!("{what}: aplicado."),
        Err(e) => {
            ok = false;
            println!("{what}: não aplicado ({}).", explain(&e));
        }
    };

    report(format!("Limite de energia {:.0} W", plan.power_mw as f32 / 1000.0), d.set_power_management_limit(plan.power_mw));
    // offsets: só mexe se a placa permite ou se o pedido é diferente de zero (aí o erro explica)
    if info.core_offset.is_some() || plan.core_offset != 0 {
        report(format!("Offset do núcleo {:+} MHz", plan.core_offset), d.set_clock_offset(Clock::Graphics, PerformanceState::Zero, plan.core_offset));
    }
    if info.mem_offset.is_some() || plan.mem_offset != 0 {
        report(format!("Offset da memória {:+} MHz", plan.mem_offset), d.set_clock_offset(Clock::Memory, PerformanceState::Zero, plan.mem_offset));
    }
    match plan.clock_cap {
        Some(max) => {
            let min = d.min_max_clock_of_pstate(Clock::Graphics, PerformanceState::Eight).map(|m| m.0).unwrap_or(210).min(max);
            report(format!("Teto de clock {max} MHz"), d.set_gpu_locked_clocks(GpuLockedClocksSetting::Numeric { min_clock_mhz: min, max_clock_mhz: max }));
        }
        None => match d.reset_gpu_locked_clocks() {
            Ok(()) => println!("Teto de clock: removido."),
            Err(NvmlError::NotSupported) => println!("Teto de clock: nenhum (esta placa não permite fixar)."),
            Err(e) => {
                ok = false;
                println!("Teto de clock: não removido ({}).", explain(&e));
            }
        },
    }
    if ok {
        println!("Pronto. Os ajustes valem até reiniciar o PC.");
        0
    } else {
        1
    }
}

// ------------------------------------------------------------------ CPU

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct CpuOc {
    /// amd-pstate-epp, amd-pstate, acpi-cpufreq, intel_pstate...
    pub driver: Option<String>,
    /// Modo do amd-pstate: active, passive ou guided.
    pub amd_pstate: Option<String>,
    pub governor: Option<String>,
    pub governors: Vec<String>,
    /// Preferência de energia (EPP): performance, balance_performance...
    pub epp: Option<String>,
    pub epps: Vec<String>,
    pub boost: Option<bool>,
    /// Preferred core (o kernel põe as tarefas nos núcleos mais rápidos).
    pub prefcore: Option<bool>,
    pub min_mhz: Option<u64>,
    pub max_mhz: Option<u64>,
    /// Limite de clock configurado agora (menor que o máximo = alguém limitou).
    pub limit_mhz: Option<u64>,
}

pub fn cpu_info() -> CpuOc {
    cpu_info_from(Path::new("/sys/devices/system/cpu"))
}

/// `dir` = /sys/devices/system/cpu (parametrizado para os testes).
pub fn cpu_info_from(dir: &Path) -> CpuOc {
    let f = dir.join("cpu0/cpufreq");
    let list = |p: PathBuf| read_trim(p).map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_default();
    let mhz = |name: &str| read_u64(f.join(name)).map(|k| k / 1000);
    let boost = read_trim(f.join("boost")).or_else(|| read_trim(dir.join("cpufreq/boost")));
    CpuOc {
        driver: read_trim(f.join("scaling_driver")),
        amd_pstate: read_trim(dir.join("amd_pstate/status")),
        governor: read_trim(f.join("scaling_governor")),
        governors: list(f.join("scaling_available_governors")),
        epp: read_trim(f.join("energy_performance_preference")),
        epps: list(f.join("energy_performance_available_preferences")),
        boost: boost.map(|b| b == "1"),
        prefcore: read_trim(f.join("amd_pstate_hw_prefcore")).map(|v| v == "enabled"),
        min_mhz: mhz("cpuinfo_min_freq"),
        max_mhz: mhz("amd_pstate_max_freq").or_else(|| mhz("cpuinfo_max_freq")),
        limit_mhz: mhz("scaling_max_freq"),
    }
}

// ------------------------------------------------------------------ erros de hardware no log do kernel

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct KernelErrors {
    /// Erros da GPU (Xid da NVIDIA, reset de anel do amdgpu): driver de vídeo travou ou reiniciou.
    pub gpu: u32,
    /// Machine Check (CPU, cache, memória, PCIe): o "WHEA" do Linux.
    pub mce: u32,
    pub last: Option<String>,
    /// Não deu para ler o log.
    pub error: Option<String>,
}

impl KernelErrors {
    pub fn total(&self) -> u32 {
        self.gpu + self.mce
    }
}

const GPU_PATTERNS: &[&str] = &["NVRM: Xid", "GPU has fallen off the bus", "ring gfx timeout", "GPU reset begin"];
const MCE_PATTERNS: &[&str] = &["Hardware Error", "Machine check events logged", "Machine Check Exception"];

/// Conta os erros num trecho do log do kernel (`journalctl -o short-iso`).
pub fn parse_kernel_log(text: &str) -> KernelErrors {
    let mut k = KernelErrors::default();
    for line in text.lines() {
        let gpu = GPU_PATTERNS.iter().any(|p| line.contains(p));
        let mce = !gpu && MCE_PATTERNS.iter().any(|p| line.contains(p));
        if gpu {
            k.gpu += 1;
        } else if mce {
            k.mce += 1;
        } else {
            continue;
        }
        // "2026-10-24T12:00:46-03:00 host kernel: mensagem" → "24/10 12:00 · mensagem"
        let mut parts = line.splitn(2, " kernel: ");
        let (stamp, msg) = (parts.next().unwrap_or(""), parts.next().unwrap_or(line));
        let when = stamp.get(8..10).zip(stamp.get(5..7)).zip(stamp.get(11..16)).map(|((d, m), t)| format!("{d}/{m} {t} · "));
        k.last = Some(format!("{}{}", when.unwrap_or_default(), msg.trim()));
    }
    k
}

/// Erros de hardware desde `since` (segundos Unix), lidos do journal (o usuário do grupo wheel pode ler).
pub fn kernel_errors_since(since: u64) -> KernelErrors {
    let pattern = GPU_PATTERNS.iter().chain(MCE_PATTERNS).copied().collect::<Vec<_>>().join("|");
    let out = Command::new("journalctl")
        .args(["_TRANSPORT=kernel", "--since", &format!("@{since}"), "-g", &pattern, "-o", "short-iso", "--no-pager", "-q"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output();
    match out {
        Err(e) => KernelErrors { error: Some(format!("journalctl indisponível: {e}")), ..Default::default() },
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            // sem nada encontrado o journalctl sai com 1 e não escreve nada
            if stdout.trim().is_empty() && !stderr.trim().is_empty() && !stderr.contains("No entries") {
                return KernelErrors { error: Some(stderr.lines().next().unwrap_or("").trim().to_string()), ..Default::default() };
            }
            parse_kernel_log(&stdout)
        }
    }
}

pub fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ------------------------------------------------------------------ verificador de estabilidade

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// CPU + RAM (carga do próprio app, com verificação da memória).
    Cpu,
    /// GPU (reset-gpu burn).
    Gpu,
    Both,
    /// Sem carga: só observa enquanto você roda um jogo ou benchmark.
    Watch,
}

impl Target {
    fn cpu(self) -> bool {
        matches!(self, Target::Cpu | Target::Both)
    }
    fn gpu(self) -> bool {
        matches!(self, Target::Gpu | Target::Both)
    }
    fn watches_gpu(self) -> bool {
        self != Target::Cpu
    }
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub target: Target,
    pub seconds: u32,
    pub gpu_index: u32,
    /// Programa de carga da GPU (`reset-gpu`). Obrigatório para `Gpu`/`Both`.
    pub reset_gpu: Option<PathBuf>,
}

/// Uma leitura por segundo durante o teste.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sample {
    pub gpu_temp: Option<f32>,
    pub gpu_core: Option<f32>,
    pub gpu_mem: Option<f32>,
    pub gpu_load: Option<f32>,
    pub perfcap: Vec<String>,
    pub cpu_temp: Option<f64>,
    pub cpu_mhz: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CheckProgress {
    pub phase: String,
    pub elapsed_s: u32,
    pub total_s: u32,
    pub now: Sample,
    pub max_gpu_temp: Option<f32>,
    pub max_cpu_temp: Option<f64>,
    pub fps: Option<f32>,
    pub mem_errors: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub stable: bool,
    pub cancelled: bool,
    /// Por que deu instável.
    pub reason: Option<String>,
    /// O que se viu: limitadores, queda de clock...
    pub notes: Vec<String>,
    pub rows: Vec<(String, String)>,
    pub seconds: u32,
}

/// Uso da GPU a partir do qual a leitura conta como "sob carga".
const LOADED_PCT: f32 = 80.0;

/// Acumula as leituras e tira as conclusões (separado do laço para dar para testar).
#[derive(Debug, Default)]
pub struct CheckStats {
    pub samples: Vec<Sample>,
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

impl CheckStats {
    pub fn push(&mut self, s: Sample) {
        self.samples.push(s);
    }

    pub fn max_gpu_temp(&self) -> Option<f32> {
        self.samples.iter().filter_map(|s| s.gpu_temp).reduce(f32::max)
    }

    pub fn max_cpu_temp(&self) -> Option<f64> {
        self.samples.iter().filter_map(|s| s.cpu_temp).reduce(f64::max)
    }

    fn loaded(&self) -> Vec<&Sample> {
        self.samples.iter().filter(|s| s.gpu_load.is_some_and(|l| l >= LOADED_PCT)).collect()
    }

    /// % do tempo sob carga em que cada limitador esteve ativo, do mais frequente para o menos.
    pub fn limiters(&self) -> Vec<(String, f32)> {
        let loaded = self.loaded();
        if loaded.is_empty() {
            return Vec::new();
        }
        let mut names: Vec<String> = loaded.iter().flat_map(|s| s.perfcap.iter().cloned()).collect();
        names.sort();
        names.dedup();
        let mut out: Vec<(String, f32)> = names
            .into_iter()
            .filter(|n| n != "Ocioso")
            .map(|n| {
                let c = loaded.iter().filter(|s| s.perfcap.contains(&n)).count();
                (n, 100.0 * c as f32 / loaded.len() as f32)
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// Clock do núcleo sob carga: (início, fim, média, mínimo). Início e fim = mediana do primeiro e do último quinto.
    pub fn gpu_clock(&self) -> Option<(f32, f32, f32, f32)> {
        let c: Vec<f32> = self.loaded().iter().filter_map(|s| s.gpu_core).collect();
        if c.len() < 3 {
            return None;
        }
        let n = (c.len() / 5).max(1);
        let first = median(c[..n].to_vec())?;
        let last = median(c[c.len() - n..].to_vec())?;
        let avg = c.iter().sum::<f32>() / c.len() as f32;
        let min = c.iter().copied().fold(f32::MAX, f32::min);
        Some((first, last, avg, min))
    }

    pub fn notes(&self, target: Target) -> Vec<String> {
        let mut out = Vec::new();
        if target.watches_gpu() {
            let loaded = self.loaded().len();
            if loaded < 3 {
                let avg = median(self.samples.iter().filter_map(|s| s.gpu_load).collect());
                out.push(format!(
                    "A GPU quase não ficou carregada (uso típico {}). {}",
                    avg.map_or("—".into(), |a| format!("{a:.0}%")),
                    if target == Target::Watch { "Abra o jogo ou benchmark antes de começar." } else { "O teste de GPU pode não ter rodado direito." }
                ));
            }
            for (name, pct) in self.limiters() {
                let txt = match name.as_str() {
                    "Energia" if pct >= 50.0 => format!("Limitada por energia em {pct:.0}% do tempo sob carga: normal em placas com limite baixo. Subir o limite de energia é o que libera clock."),
                    "Térmico" | "Térmico (HW)" if pct >= 5.0 => format!("Limitada por temperatura em {pct:.0}% do tempo: melhore a ventilação ou reduza o limite/offset."),
                    "Freio de energia" => format!("Freio de energia do hardware acionado ({pct:.0}% do tempo): pode ser a fonte ou o cabo de energia da placa."),
                    "Clock fixado" => "Teto de clock ativo: o clock não passa do valor escolhido (esperado se você aplicou um teto).".to_string(),
                    _ => continue,
                };
                out.push(txt);
            }
            if let Some((first, last, avg, min)) = self.gpu_clock() {
                if last < first * 0.95 {
                    out.push(format!("O clock caiu de {first:.0} para {last:.0} MHz ao longo do teste (aquecimento ou limite de energia)."));
                } else {
                    out.push(format!("Clock firme sob carga: média {avg:.0} MHz, mínimo {min:.0} MHz."));
                }
            }
        }
        out
    }
}

/// Motivo de parar por temperatura (o primeiro limite atingido).
pub fn temp_fail(s: &Sample, target: Target) -> Option<String> {
    if target.cpu() {
        if let Some(t) = s.cpu_temp.filter(|t| *t >= CPU_LIMIT_C) {
            return Some(format!("CPU chegou a {t:.0} °C (limite de segurança {CPU_LIMIT_C:.0} °C)"));
        }
    }
    if target.watches_gpu() {
        if let Some(t) = s.gpu_temp.filter(|t| *t >= GPU_LIMIT_C) {
            return Some(format!("GPU chegou a {t:.0} °C (limite de segurança {GPU_LIMIT_C:.0} °C)"));
        }
    }
    None
}

pub fn kernel_fail(k: &KernelErrors) -> Option<String> {
    (k.total() > 0).then(|| format!("o kernel registrou {} erro(s) de GPU (Xid) e {} de hardware (MCE)", k.gpu, k.mce))
}

/// Carga de CPU em todos os threads menos um, e no último um teste de memória (escreve um padrão, confere,
/// repete com outro). Erros de memória com overclock/EXPO instável aparecem aqui.
fn start_cpu_ram(stop: Arc<AtomicBool>, errors: Arc<AtomicU64>) -> Vec<std::thread::JoinHandle<()>> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).max(2);
    let mut hs = Vec::new();
    for t in 0..threads - 1 {
        let stop = stop.clone();
        hs.push(std::thread::spawn(move || {
            let mut x = 1.0f64 + t as f64;
            while !stop.load(Ordering::Relaxed) {
                for _ in 0..2_000_000 {
                    x = std::hint::black_box((x * 1.000_000_1 + 0.5).sqrt() + x.sin() * 1e-3);
                }
            }
        }));
    }
    let avail = crate::system::memory().available;
    let bytes = (avail / 4).clamp(64 << 20, 2 << 30);
    hs.push(std::thread::spawn(move || {
        let mut buf = vec![0u64; (bytes / 8) as usize];
        let mut pass: u64 = 1;
        'outer: while !stop.load(Ordering::Relaxed) {
            let seed = pass.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            for (i, chunk) in buf.chunks_mut(1 << 20).enumerate() {
                if stop.load(Ordering::Relaxed) {
                    break 'outer;
                }
                let base = (i << 20) as u64;
                for (j, v) in chunk.iter_mut().enumerate() {
                    *v = (base + j as u64) ^ seed;
                }
            }
            let mut bad = 0;
            for (i, v) in buf.iter().enumerate() {
                if std::hint::black_box(*v) != (i as u64) ^ seed {
                    bad += 1;
                }
            }
            errors.fetch_add(bad, Ordering::Relaxed);
            pass += 1;
        }
    }));
    hs
}

/// Carga da GPU: `reset-gpu burn` numa janela (contrato no linux/README.md). Lê o FPS e os erros.
struct GpuLoad {
    child: std::process::Child,
    fps: Arc<Mutex<Option<f32>>>,
    error: Arc<Mutex<Option<String>>>,
}

impl GpuLoad {
    fn start(exe: &Path, seconds: u32) -> Result<Self, String> {
        let mut child = Command::new(exe)
            .args(["burn", "--width", "1280", "--height", "720", "--msaa", "4", "--intensity", "alta"])
            .args(["--seconds", &(seconds + 5).to_string(), "--temp-limit", &format!("{GPU_LIMIT_C:.0}")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("não deu para abrir o teste de GPU: {e}"))?;
        let fps = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let out = child.stdout.take().unwrap();
        let (f, er) = (fps.clone(), error.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                match v["type"].as_str() {
                    Some("progress") => *f.lock().unwrap() = v["fps"].as_f64().map(|x| x as f32),
                    Some("error") => *er.lock().unwrap() = Some(v["message"].as_str().unwrap_or("erro").to_string()),
                    Some("result") if v["stopped"] == "temperatura" => *er.lock().unwrap() = Some("parou por temperatura".into()),
                    _ => {}
                }
            }
        });
        Ok(Self { child, fps, error })
    }

    /// Motivo de falha se o programa saiu antes da hora.
    fn died(&mut self) -> Option<String> {
        let status = self.child.try_wait().ok()??;
        let err = self.error.lock().unwrap().clone();
        Some(match err {
            Some(e) => format!("a GPU falhou ({e})"),
            None if status.success() => "o teste de GPU terminou antes da hora".into(),
            None => "a janela do teste de GPU foi fechada".into(),
        })
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Roda o verificador. `progress` é chamado a cada segundo; `cancel` interrompe.
pub fn run_check(opts: &CheckOptions, cancel: &AtomicBool, mut progress: impl FnMut(&CheckProgress)) -> CheckResult {
    let started = now_s();
    let clock = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let mem_errors = Arc::new(AtomicU64::new(0));
    let mut stats = CheckStats::default();
    let mut fail: Option<String> = None;
    let mut p = CheckProgress { total_s: opts.seconds, ..Default::default() };

    let mut gpu_load = None;
    if opts.target.gpu() {
        match opts.reset_gpu.as_deref().map(|exe| GpuLoad::start(exe, opts.seconds)) {
            Some(Ok(g)) => gpu_load = Some(g),
            Some(Err(e)) => fail = Some(e),
            None => fail = Some("o programa de carga da GPU (reset-gpu) não foi encontrado".into()),
        }
    }
    let workers = if opts.target.cpu() && fail.is_none() { start_cpu_ram(stop.clone(), mem_errors.clone()) } else { Vec::new() };
    p.phase = match opts.target {
        Target::Cpu => "Stress CPU + RAM",
        Target::Gpu => "Stress GPU",
        Target::Both => "Stress CPU + RAM + GPU",
        Target::Watch => "Observando",
    }
    .into();

    let mut sensors = SensorMonitor::new();
    sensors.poll(); // primeira leitura só serve de base para a carga da CPU
    let mut kernel = KernelErrors::default();
    while fail.is_none() && !cancel.load(Ordering::Relaxed) && clock.elapsed() < Duration::from_secs(opts.seconds as u64) {
        std::thread::sleep(Duration::from_secs(1));
        let snap = sensors.poll().summary;
        let g = if opts.target.watches_gpu() { gpu::live(opts.gpu_index) } else { None };
        let s = Sample {
            gpu_temp: g.as_ref().and_then(|g| g.temp_c),
            gpu_core: g.as_ref().and_then(|g| g.core_mhz),
            gpu_mem: g.as_ref().and_then(|g| g.mem_mhz),
            gpu_load: g.as_ref().and_then(|g| g.load_pct),
            perfcap: g.map(|g| g.perfcap).unwrap_or_default(),
            cpu_temp: snap.cpu_temp_c,
            cpu_mhz: snap.cpu_mhz_max,
        };
        fail = temp_fail(&s, opts.target);
        stats.push(s.clone());
        p.elapsed_s = clock.elapsed().as_secs() as u32;
        p.now = s;
        p.max_gpu_temp = stats.max_gpu_temp();
        p.max_cpu_temp = stats.max_cpu_temp();
        p.mem_errors = mem_errors.load(Ordering::Relaxed);
        if let Some(g) = gpu_load.as_mut() {
            p.fps = *g.fps.lock().unwrap();
            if fail.is_none() {
                fail = g.died();
            }
        }
        if fail.is_none() && p.mem_errors > 0 {
            fail = Some(format!("{} erro(s) de memória (RAM/EXPO instável)", p.mem_errors));
        }
        if fail.is_none() && p.elapsed_s.is_multiple_of(5) {
            kernel = kernel_errors_since(started);
            fail = kernel_fail(&kernel);
        }
        progress(&p);
    }

    stop.store(true, Ordering::Relaxed);
    if let Some(g) = gpu_load.as_mut() {
        g.stop();
    }
    for w in workers {
        let _ = w.join();
    }
    let cancelled = cancel.load(Ordering::Relaxed) && fail.is_none();
    let seconds = clock.elapsed().as_secs() as u32;
    if fail.is_none() {
        kernel = kernel_errors_since(started);
        fail = kernel_fail(&kernel);
    }
    if fail.is_none() && opts.target == Target::Gpu && stats.loaded().len() < 3 && !cancelled {
        fail = Some("a GPU não ficou carregada: o teste de GPU não rodou direito".into());
    }

    let mut rows = Vec::new();
    let mut row = |k: &str, v: String| rows.push((k.to_string(), v));
    if let Some(t) = stats.max_cpu_temp() {
        row("Temp. máx. CPU", format!("{t:.0} °C"));
    }
    if let Some(t) = stats.max_gpu_temp() {
        row("Temp. máx. GPU", format!("{t:.0} °C"));
    }
    if let Some((_, _, avg, min)) = stats.gpu_clock() {
        row("Clock da GPU sob carga", format!("média {avg:.0} MHz · mín. {min:.0} MHz"));
    }
    if let Some(m) = median(stats.loaded().iter().filter_map(|s| s.gpu_mem).collect()) {
        row("Clock da memória da GPU", format!("{m:.0} MHz"));
    }
    if let Some(f) = p.fps {
        row("FPS do teste (último)", format!("{f:.0}"));
    }
    let lim = stats.limiters();
    if !lim.is_empty() {
        row("Limitadores sob carga", lim.iter().map(|(n, p)| format!("{n} {p:.0}%")).collect::<Vec<_>>().join(" · "));
    }
    if opts.target.cpu() {
        row("Erros de memória", p.mem_errors.to_string());
    }
    row(
        "Erros do kernel (GPU / MCE)",
        kernel.error.clone().map_or(format!("{} / {}", kernel.gpu, kernel.mce), |e| format!("não deu para ler ({e})")),
    );

    CheckResult { stable: fail.is_none() && !cancelled, cancelled, reason: fail, notes: stats.notes(opts.target), rows, seconds }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn limits() -> Limits {
        let o = |min, max| Some(OffsetInfo { current: 0, min, max });
        Limits::new((90.0, 130.0, 115.0), o(-1000, 1000), o(-2000, 6000), 3105)
    }

    fn req(power_w: f32, core: i32, mem: i32, cap: Option<u32>) -> OcRequest {
        OcRequest { index: 0, power_w, core_offset: core, mem_offset: mem, clock_cap: cap }
    }

    #[test]
    fn faixa_do_driver_cruzada_com_a_segura() {
        let l = limits();
        assert_eq!(l.core, Some((-500, 300)));
        assert_eq!(l.mem, Some((-1000, 1500)));
        assert_eq!((l.cap_min, l.cap_max), (1552, 3105));
        // driver com faixa menor que a segura: vale a do driver
        let l = Limits::new((90.0, 130.0, 115.0), Some(OffsetInfo { current: 0, min: -200, max: 150 }), None, 3000);
        assert_eq!(l.core, Some((-200, 150)));
        assert_eq!(l.mem, None);
    }

    #[test]
    fn valida_energia() {
        let l = limits();
        assert_eq!(validate(&req(120.0, 0, 0, None), &l).unwrap().power_mw, 120_000);
        assert_eq!(validate(&req(130.4, 0, 0, None), &l).unwrap().power_mw, 130_000); // arredondamento do controle
        assert!(validate(&req(131.0, 0, 0, None), &l).is_err());
        assert!(validate(&req(80.0, 0, 0, None), &l).is_err());
        assert!(validate(&req(f32::NAN, 0, 0, None), &l).is_err());
    }

    #[test]
    fn valida_offsets_e_teto() {
        let l = limits();
        assert!(validate(&req(115.0, 150, 1000, Some(2800)), &l).is_ok());
        assert!(validate(&req(115.0, 301, 0, None), &l).is_err());
        assert!(validate(&req(115.0, -501, 0, None), &l).is_err());
        assert!(validate(&req(115.0, 0, 1501, None), &l).is_err());
        assert!(validate(&req(115.0, 0, 0, Some(4000)), &l).is_err());
        assert!(validate(&req(115.0, 0, 0, Some(500)), &l).is_err());
        // placa sem offset: zero passa, qualquer outro valor não
        let sem = Limits { core: None, mem: None, ..l };
        assert!(validate(&req(115.0, 0, 0, None), &sem).is_ok());
        assert!(validate(&req(115.0, 15, 0, None), &sem).is_err());
    }

    #[test]
    fn restaurar_e_o_padrao() {
        let l = limits();
        let p = validate(&reset_request(0, &l), &l).unwrap();
        assert_eq!(p, Plan { index: 0, power_mw: 115_000, core_offset: 0, mem_offset: 0, clock_cap: None });
    }

    #[test]
    fn argumentos_ida_e_volta() {
        let p = Plan { index: 0, power_mw: 125_000, core_offset: -45, mem_offset: 750, clock_cap: Some(2600) };
        let args = p.args();
        assert_eq!(args[0], CLI_FLAG);
        assert_eq!(Plan::parse(&args[1..]).unwrap(), p);
        let p = Plan { clock_cap: None, ..p };
        assert_eq!(Plan::parse(&p.args()).unwrap(), p);
        let bad = |a: &[&str]| Plan::parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err();
        assert!(bad(&["--gpu", "0"]));
        assert!(bad(&["--gpu", "0", "--power-mw", "-5", "--core-offset", "0", "--mem-offset", "0", "--clock-cap", "none"]));
        assert!(bad(&["--gpu", "0", "--power-mw", "1", "--core-offset", "x", "--mem-offset", "0", "--clock-cap", "none"]));
        assert!(bad(&["--gpu", "0", "--power-mw", "1", "--core-offset", "0", "--mem-offset", "0", "--clock-cap", "1;rm"]));
    }

    #[test]
    fn log_do_kernel() {
        let log = "2026-10-24T09:45:25-03:00 pc kernel: MCE: In-kernel MCE decoding enabled.\n\
                   2026-10-24T10:01:02-03:00 pc kernel: NVRM: Xid (PCI:0000:2b:00): 79, GPU has fallen off the bus.\n\
                   2026-10-24T11:15:40-03:00 pc kernel: mce: [Hardware Error]: Machine check events logged\n";
        let k = parse_kernel_log(log);
        assert_eq!((k.gpu, k.mce), (1, 1));
        assert_eq!(k.last.as_deref(), Some("24/10 11:15 · mce: [Hardware Error]: Machine check events logged"));
        assert_eq!(parse_kernel_log("").total(), 0);
        assert!(kernel_fail(&k).is_some());
    }

    #[test]
    fn cpu_pelo_sysfs() {
        let dir = std::env::temp_dir().join(format!("re-oc-cpu-{}", std::process::id()));
        let f = dir.join("cpu0/cpufreq");
        fs::create_dir_all(&f).unwrap();
        fs::create_dir_all(dir.join("amd_pstate")).unwrap();
        for (k, v) in [
            ("scaling_driver", "amd-pstate-epp"),
            ("scaling_governor", "performance"),
            ("scaling_available_governors", "performance powersave"),
            ("energy_performance_preference", "performance"),
            ("boost", "1"),
            ("amd_pstate_hw_prefcore", "enabled"),
            ("amd_pstate_max_freq", "4665835"),
            ("cpuinfo_min_freq", "562149"),
            ("scaling_max_freq", "4665835"),
        ] {
            fs::write(f.join(k), format!("{v}\n")).unwrap();
        }
        fs::write(dir.join("amd_pstate/status"), "active\n").unwrap();
        let c = cpu_info_from(&dir);
        assert_eq!(c.driver.as_deref(), Some("amd-pstate-epp"));
        assert_eq!(c.amd_pstate.as_deref(), Some("active"));
        assert_eq!(c.governors, vec!["performance", "powersave"]);
        assert_eq!((c.boost, c.prefcore), (Some(true), Some(true)));
        assert_eq!((c.max_mhz, c.min_mhz, c.limit_mhz), (Some(4665), Some(562), Some(4665)));
        assert!(c.epps.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    fn sample(load: f32, core: f32, temp: f32, cap: &[&str]) -> Sample {
        Sample { gpu_load: Some(load), gpu_core: Some(core), gpu_temp: Some(temp), perfcap: cap.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn limitadores_e_queda_de_clock() {
        let mut st = CheckStats::default();
        st.push(sample(5.0, 210.0, 40.0, &["Ocioso"])); // fora da carga: não conta
        for i in 0..10 {
            let core = if i < 5 { 2800.0 } else { 2500.0 };
            let cap: &[&str] = if i % 2 == 0 { &["Energia"] } else { &["Energia", "Térmico"] };
            st.push(sample(99.0, core, 70.0 + i as f32, cap));
        }
        let lim = st.limiters();
        assert_eq!(lim[0], ("Energia".to_string(), 100.0));
        assert_eq!(lim[1], ("Térmico".to_string(), 50.0));
        let (first, last, _, min) = st.gpu_clock().unwrap();
        assert_eq!((first, last, min), (2800.0, 2500.0, 2500.0));
        assert_eq!(st.max_gpu_temp(), Some(79.0));
        let notes = st.notes(Target::Gpu).join("\n");
        assert!(notes.contains("energia em 100%"));
        assert!(notes.contains("temperatura em 50%"));
        assert!(notes.contains("caiu de 2800 para 2500"));
    }

    #[test]
    fn observar_sem_jogo_avisa() {
        let mut st = CheckStats::default();
        for _ in 0..5 {
            st.push(sample(3.0, 210.0, 40.0, &["Ocioso"]));
        }
        assert!(st.notes(Target::Watch)[0].contains("Abra o jogo"));
        assert!(st.notes(Target::Cpu).is_empty());
    }

    #[test]
    fn limites_de_temperatura() {
        let hot_gpu = Sample { gpu_temp: Some(88.0), cpu_temp: Some(60.0), ..Default::default() };
        assert!(temp_fail(&hot_gpu, Target::Gpu).unwrap().contains("GPU chegou a 88"));
        assert!(temp_fail(&hot_gpu, Target::Cpu).is_none()); // GPU não é olhada no teste só de CPU
        let hot_cpu = Sample { cpu_temp: Some(96.0), ..Default::default() };
        assert!(temp_fail(&hot_cpu, Target::Both).unwrap().contains("CPU chegou a 96"));
        assert!(temp_fail(&hot_cpu, Target::Watch).is_none());
    }
}
