//! Stress test, portado do `StressTester.cs` e da página de stress do app do Windows: carga de CPU em
//! todas as threads (ponto flutuante + inteiros), RAM (grava e verifica padrões numa fração da memória
//! livre) e GPU (`reset-gpu burn`, estilo FurMark). Para sozinho pelo limite de temperatura da CPU ou da
//! GPU, por erro de memória, pela janela da GPU fechada ou pelo tempo escolhido.
//!
//! `Stress` guarda tudo (teste em andamento, histórico de 5 min, registro de eventos); quem usa chama
//! `tick` uma vez por segundo com uma leitura dos sensores (`read_sample`), e é aí que os limites valem.

use crate::bench::gpu::{self as gpu_tool, GpuEvent, GpuResult};
use crate::sensors::{Kind, SensorMonitor};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 5 min a 1 amostra/s.
pub const HISTORY_LEN: usize = 300;
/// 128 MB por bloco de RAM.
const CHUNK_LONGS: usize = 16 * 1024 * 1024;

// ------------------------------------------------------------------ carga de CPU e RAM

/// Contadores do teste de RAM, lidos pela interface enquanto roda.
#[derive(Default)]
pub struct RamStats {
    pub errors: AtomicU64,
    pub passes: AtomicU64,
    pub allocated: AtomicU64,
}

/// Threads de carga em andamento.
pub struct Tester {
    cancel: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    pub ram: Arc<RamStats>,
    pub cpu: bool,
}

impl Tester {
    /// `cpu`: carga em todas as threads. `ram_bytes`: quanto alocar e verificar (0 = sem RAM).
    pub fn start(cpu: bool, ram_bytes: u64) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let ram = Arc::new(RamStats::default());
        let mut threads = Vec::new();
        let mut n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        // Com RAM ativa, reserva um thread para o worker de memória.
        if cpu && ram_bytes > 0 && n > 1 {
            n -= 1;
        }
        if cpu {
            for i in 0..n {
                let c = cancel.clone();
                threads.push(spawn(format!("Stress CPU {i}"), move || cpu_worker(&c, i as u64)));
            }
        }
        if ram_bytes > 0 {
            let (c, r) = (cancel.clone(), ram.clone());
            ram.allocated.store(ram_bytes, Ordering::Relaxed);
            threads.push(spawn("Stress RAM".into(), move || ram_worker(&c, ram_bytes, CHUNK_LONGS, &r)));
        }
        Self { cancel, threads, ram, cpu }
    }

    pub fn stop(self) {
        self.cancel.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

/// Prioridade baixa (nice 10) mantém a janela responsiva sem reduzir a carga (núcleos continuam a 100%).
fn spawn(name: String, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            unsafe { libc::setpriority(libc::PRIO_PROCESS as _, libc::gettid() as _, 10) };
            f()
        })
        .expect("não deu para criar thread do stress test")
}

/// Mistura ponto flutuante (FMA/sqrt/sin) e inteiros sobre um buffer que cabe no cache L1/L2.
pub fn cpu_worker(cancel: &AtomicBool, seed: u64) {
    let mut buf: Vec<f64> = (0..4096).map(|i| (i as u64 + seed + 1) as f64).collect();
    let mut x = (seed + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    while !cancel.load(Ordering::Relaxed) {
        for _ in 0..64 {
            for (i, b) in buf.iter_mut().enumerate() {
                let mut v = *b;
                v = v.mul_add(1.0000001, 0.5);
                v = (v * v + 1.0).sqrt() + v.sin() * 1e-3;
                *b = if v > 1e12 { (i + 1) as f64 } else { v };
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
            }
        }
        std::hint::black_box((&buf, x)); // evita que o compilador elimine o cálculo inteiro
    }
}

/// Padrão de uma passada: muda a cada passada para pegar bits que "grudam".
pub fn pattern(pass: u64) -> u64 {
    pass.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

pub fn fill(chunk: &mut [u64], pattern: u64) {
    for (i, v) in chunk.iter_mut().enumerate() {
        *v = pattern ^ i as u64;
    }
}

/// Quantos valores não batem com o padrão gravado.
pub fn verify(chunk: &[u64], pattern: u64) -> u64 {
    chunk.iter().enumerate().filter(|(i, v)| **v != pattern ^ *i as u64).count() as u64
}

/// Aloca `total` bytes em blocos e grava/verifica padrões até cancelar. Se faltar memória, segue com o que conseguiu.
pub fn ram_worker(cancel: &AtomicBool, total: u64, chunk_longs: usize, stats: &RamStats) {
    let mut chunks: Vec<Vec<u64>> = Vec::new();
    let mut allocated = 0u64;
    while allocated + chunk_longs as u64 * 8 <= total && !cancel.load(Ordering::Relaxed) {
        let mut c = Vec::new();
        if c.try_reserve_exact(chunk_longs).is_err() {
            break;
        }
        c.resize(chunk_longs, 0);
        chunks.push(c);
        allocated += chunk_longs as u64 * 8;
    }
    stats.allocated.store(allocated, Ordering::Relaxed);
    let mut pass = 0u64;
    while !chunks.is_empty() && !cancel.load(Ordering::Relaxed) {
        pass += 1;
        let p = pattern(pass);
        for c in &mut chunks {
            fill(c, p);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
        }
        // o compilador não pode supor que a memória continua igual ao que acabou de gravar
        std::hint::black_box(&mut chunks);
        for c in &chunks {
            let errs = verify(c, p);
            if errs > 0 {
                stats.errors.fetch_add(errs, Ordering::Relaxed);
            }
            if cancel.load(Ordering::Relaxed) {
                return;
            }
        }
        stats.passes.fetch_add(1, Ordering::Relaxed);
    }
}

/// Bytes para o teste de RAM: fração (10–90%) da memória disponível (MemAvailable).
pub fn ram_bytes_for(fraction: f64) -> u64 {
    (crate::system::memory().available as f64 * fraction.clamp(0.1, 0.9)) as u64
}

// ------------------------------------------------------------------ GPU (reset-gpu burn)

#[derive(Default)]
struct BurnLive {
    fps: Option<f64>,
    fps_sum: f64,
    fps_n: u64,
    adapter: Option<String>,
    temp_c: Option<f64>,
    power_w: Option<f64>,
}

struct Burn {
    cancel: Arc<AtomicBool>,
    live: Arc<Mutex<BurnLive>>,
    handle: Option<JoinHandle<Result<Option<GpuResult>, String>>>,
    /// Como terminou, quando já terminou.
    outcome: Option<Result<Option<GpuResult>, String>>,
}

impl Burn {
    fn start(args: Vec<String>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let live = Arc::new(Mutex::new(BurnLive::default()));
        let (c, l) = (cancel.clone(), live.clone());
        // a thread que inicia o reset-gpu espera por ele: se o app fechar, o programa fecha junto
        let handle = std::thread::Builder::new()
            .name("Stress GPU".into())
            .spawn(move || {
                gpu_tool::run(&args, &c, &mut |ev| {
                    let mut l = l.lock().unwrap();
                    match ev {
                        GpuEvent::Info { adapter, .. } => l.adapter = adapter.clone(),
                        GpuEvent::Progress { fps, temp_c, power_w, .. } => {
                            l.fps = *fps;
                            l.temp_c = *temp_c;
                            l.power_w = *power_w;
                            if let Some(f) = fps {
                                l.fps_sum += f;
                                l.fps_n += 1;
                            }
                        }
                        _ => {}
                    }
                })
            })
            .ok();
        Self { cancel, live, handle, outcome: None }
    }

    /// true enquanto o reset-gpu está rodando; quando termina, guarda como terminou.
    fn alive(&mut self) -> bool {
        if self.outcome.is_some() {
            return false;
        }
        match &self.handle {
            Some(h) if !h.is_finished() => true,
            _ => {
                self.outcome = Some(match self.handle.take().map(|h| h.join()) {
                    Some(Ok(r)) => r,
                    _ => Err("a thread da GPU falhou".into()),
                });
                false
            }
        }
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            self.outcome = Some(h.join().unwrap_or_else(|_| Err("a thread da GPU falhou".into())));
        }
    }

    fn avg_fps(&self) -> Option<f64> {
        if let Some(Ok(Some(r))) = &self.outcome {
            if r.avg_fps.is_some() {
                return r.avg_fps;
            }
        }
        let l = self.live.lock().unwrap();
        (l.fps_n > 0).then(|| l.fps_sum / l.fps_n as f64)
    }

    fn frames(&self) -> Option<u64> {
        match &self.outcome {
            Some(Ok(Some(r))) => r.frames,
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ sessão

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub cpu: bool,
    pub ram: bool,
    /// Fração da RAM livre (0,4 / 0,6 / 0,8).
    pub ram_fraction: f64,
    pub cpu_limit: f64,
    pub gpu: bool,
    pub gpu_limit: f64,
    /// 0 = sem limite.
    pub minutes: f64,
    /// "1280x720", "1920x1080", "2560x1440" ou "full".
    pub gpu_res: String,
    /// 0, 4 ou 8.
    pub gpu_msaa: u32,
    /// "normal", "alta" ou "extrema".
    pub gpu_intensity: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cpu: true,
            ram: false,
            ram_fraction: 0.6,
            // Ryzen 5000/7000 trabalham em 90 °C (Tjmax) por projeto sob carga total; Intel vai até 100.
            // Com 90 (o padrão do Windows) o teste pararia logo no começo num Ryzen.
            cpu_limit: 95.0,
            gpu: false,
            gpu_limit: 85.0,
            minutes: 10.0,
            gpu_res: "1920x1080".into(),
            gpu_msaa: 4,
            gpu_intensity: "alta".into(),
        }
    }
}

impl Config {
    /// Mesmas regras do Windows. Devolve a mensagem para o usuário se algo estiver fora.
    pub fn validate(&self) -> Result<(), String> {
        if !self.cpu && !self.ram && !self.gpu {
            return Err("Ative CPU, RAM ou GPU.".into());
        }
        if !(40.0..=110.0).contains(&self.cpu_limit) {
            return Err("Limite da CPU deve estar entre 40 e 110 °C.".into());
        }
        if self.gpu && !(40.0..=100.0).contains(&self.gpu_limit) {
            return Err("Limite da GPU deve estar entre 40 e 100 °C.".into());
        }
        if !(self.minutes >= 0.0 && self.minutes.is_finite()) {
            return Err("Duração inválida.".into());
        }
        Ok(())
    }

    fn burn_args(&self) -> Result<Vec<String>, String> {
        let full = self.gpu_res == "full";
        let (w, h) = if full {
            (0, 0)
        } else {
            let (w, h) = self.gpu_res.split_once('x').ok_or("resolução inválida")?;
            (w.parse().map_err(|_| "resolução inválida")?, h.parse().map_err(|_| "resolução inválida")?)
        };
        gpu_tool::burn_args(w, h, full, self.gpu_msaa, &self.gpu_intensity, (self.minutes * 60.0).round() as u64, self.gpu_limit.round() as u32)
    }

    fn gpu_desc(&self) -> String {
        let res = match self.gpu_res.as_str() {
            "full" => "tela cheia".to_string(),
            r => r.split_once('x').map(|(_, h)| format!("{h}p")).unwrap_or_else(|| r.to_string()),
        };
        let msaa = if self.gpu_msaa > 1 { format!("MSAA {}x", self.gpu_msaa) } else { "sem MSAA".into() };
        let int = match self.gpu_intensity.as_str() {
            "normal" => "Normal",
            "extrema" => "Extrema",
            _ => "Alta",
        };
        format!("sim ({res}, {msaa}, {int}, limite {} °C)", fmt0(self.gpu_limit))
    }
}

/// Uma leitura dos sensores por segundo.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sample {
    pub cpu_temp: Option<f64>,
    pub cpu_load: Option<f64>,
    pub cpu_power: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub gpu_power: Option<f64>,
    /// O que segura o clock da GPU agora ("Energia", "Térmico"...).
    pub gpu_perfcap: Option<String>,
}

/// Lê CPU (temperatura, carga, consumo) e GPU NVIDIA (temperatura, consumo, limitador).
pub struct SampleReader {
    monitor: SensorMonitor,
    rapl: Option<(u64, Instant)>,
}

impl Default for SampleReader {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleReader {
    pub fn new() -> Self {
        Self { monitor: SensorMonitor::new(), rapl: None }
    }

    pub fn read(&mut self) -> Sample {
        let snap = self.monitor.poll();
        let s = &snap.summary;
        // Consumo da CPU: RAPL (precisa de permissão de leitura no energy_uj) ou um sensor hwmon de consumo da CPU.
        let hwmon_power = snap.rows.iter().find(|r| r.hardware == "CPU" && r.kind == Kind::Power).map(|r| r.value);
        let g = s.gpu.as_ref();
        Sample {
            cpu_temp: s.cpu_temp_c,
            cpu_load: s.cpu_load_pct,
            cpu_power: self.rapl_watts().or(hwmon_power),
            gpu_temp: g.and_then(|g| g.temp_c).map(f64::from),
            gpu_power: g.and_then(|g| g.power_w).map(f64::from),
            gpu_perfcap: g.filter(|g| !g.perfcap.is_empty()).map(|g| g.perfcap.join(", ")),
        }
    }

    fn rapl_watts(&mut self) -> Option<f64> {
        let uj = crate::util::read_u64("/sys/class/powercap/intel-rapl:0/energy_uj")?;
        let now = Instant::now();
        let prev = self.rapl.replace((uj, now));
        let (puj, pt) = prev?;
        let dt = now.duration_since(pt).as_secs_f64();
        (uj >= puj && dt > 0.2).then(|| (uj - puj) as f64 / 1e6 / dt)
    }
}

struct Run {
    cfg: Config,
    start: Instant,
    tester: Option<Tester>,
    burn: Option<Burn>,
    max_cpu: f64,
    max_gpu: f64,
}

/// Estado completo do stress test (um por app).
pub struct Stress {
    run: Option<Run>,
    hist_cpu: VecDeque<f64>,
    hist_gpu: VecDeque<f64>,
    log: Vec<String>,
    last: Sample,
    status: (String, bool),
    cfg: Config,
    /// Últimos números do teste que terminou (continuam na tela depois de parar).
    done: Option<Finished>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct Finished {
    elapsed_s: f64,
    max_cpu: Option<f64>,
    max_gpu: Option<f64>,
    ram_passes: Option<u64>,
    ram_errors: Option<u64>,
    gpu_fps_avg: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub status: String,
    /// Cartão de status invertido (parada por limite ou erro).
    pub alert: bool,
    pub elapsed_s: f64,
    pub config: Config,
    pub sample: Sample,
    pub cpu_max: Option<f64>,
    pub gpu_max: Option<f64>,
    pub gpu_on: bool,
    pub gpu_fps: Option<f64>,
    pub gpu_fps_avg: Option<f64>,
    pub gpu_adapter: Option<String>,
    pub ram_on: bool,
    pub ram_allocated: u64,
    pub ram_passes: u64,
    pub ram_errors: u64,
    pub history_cpu: Vec<f64>,
    pub history_gpu: Vec<f64>,
    pub history_len: usize,
    pub log: Vec<String>,
}

impl Default for Stress {
    fn default() -> Self {
        Self::new()
    }
}

impl Stress {
    pub fn new() -> Self {
        Self {
            run: None,
            hist_cpu: VecDeque::new(),
            hist_gpu: VecDeque::new(),
            log: Vec::new(),
            last: Sample::default(),
            status: ("Parado".into(), false),
            cfg: Config::default(),
            done: None,
        }
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    /// Começa o teste. A interface já confirmou com o usuário se faltar sensor de temperatura.
    pub fn start(&mut self, cfg: Config) -> Result<(), String> {
        if self.run.is_some() {
            return Err("O stress test já está rodando.".into());
        }
        cfg.validate()?;
        let burn_args = if cfg.gpu { Some(cfg.burn_args()?) } else { None };
        if cfg.gpu && gpu_tool::find().is_none() {
            return Err(gpu_tool::NOT_FOUND.into());
        }
        let ram_bytes = if cfg.ram { ram_bytes_for(cfg.ram_fraction) } else { 0 };
        let tester = (cfg.cpu || cfg.ram).then(|| Tester::start(cfg.cpu, ram_bytes));
        let burn = burn_args.map(Burn::start);
        self.hist_cpu.clear();
        self.hist_gpu.clear();
        self.done = None;
        self.status = ("Rodando".into(), false);
        let cpu = if cfg.cpu { format!("sim (limite {} °C)", fmt0(cfg.cpu_limit)) } else { "não".into() };
        let ram = if cfg.ram { format!("sim ({}% da livre)", fmt0(cfg.ram_fraction * 100.0)) } else { "não".into() };
        let gpu = if cfg.gpu { cfg.gpu_desc() } else { "não".into() };
        let dur = if cfg.minutes > 0.0 { format!("{} min", fmt2(cfg.minutes)) } else { "sem limite de tempo".into() };
        self.add_log(format!("Iniciado · CPU {cpu} · RAM {ram} · GPU {gpu} · {dur}"));
        self.cfg = cfg.clone();
        self.run = Some(Run { cfg, start: Instant::now(), tester, burn, max_cpu: 0.0, max_gpu: 0.0 });
        Ok(())
    }

    /// Para o teste (pelo usuário ou por um limite) e registra o resumo.
    pub fn stop(&mut self, reason: &str) {
        let Some(mut run) = self.run.take() else { return };
        let elapsed = run.start.elapsed().as_secs_f64();
        let mut fin = Finished { elapsed_s: elapsed, ..Default::default() };
        let mut msg = format!("{reason} · duração {}", hms(elapsed));
        if let Some(t) = run.tester.take() {
            let (passes, errors) = (t.ram.passes.load(Ordering::Relaxed), t.ram.errors.load(Ordering::Relaxed));
            let had_ram = t.ram.allocated.load(Ordering::Relaxed) > 0;
            if t.cpu || had_ram {
                fin.max_cpu = (run.max_cpu > 0.0).then_some(run.max_cpu);
            }
            if t.cpu {
                msg += &format!(" · CPU temp. máx. {}", fmt_temp(fin.max_cpu));
            }
            if had_ram {
                fin.ram_passes = Some(passes);
                fin.ram_errors = Some(errors);
                msg += &format!(" · RAM {passes} passes, {errors} erros");
            }
            t.stop();
        }
        if let Some(mut b) = run.burn.take() {
            b.stop();
            fin.max_gpu = (run.max_gpu > 0.0).then_some(run.max_gpu);
            fin.gpu_fps_avg = b.avg_fps();
            // "Pontuação" no estilo FurMark: frames renderizados e FPS médio.
            let frames = b.frames().map(|f| format!("{} frames, ", crate::bench::fmt_num(f as f64, 0))).unwrap_or_default();
            msg += &format!(
                " · GPU: {frames}média {} FPS, temp. máx. {}",
                fin.gpu_fps_avg.map(fmt0).unwrap_or_else(|| "—".into()),
                fmt_temp(fin.max_gpu)
            );
        }
        let aborted = reason.starts_with("ABORTADO");
        self.status = (if aborted { "Abortado" } else { "Parado" }.into(), aborted);
        self.done = Some(fin);
        self.add_log(msg);
    }

    /// Uma vez por segundo: guarda o histórico e aplica os limites (temperatura, erro de RAM, GPU, tempo).
    pub fn tick(&mut self, s: Sample) {
        push(&mut self.hist_cpu, s.cpu_temp);
        push(&mut self.hist_gpu, s.gpu_temp);
        self.last = s.clone();
        let Some(run) = self.run.as_mut() else { return };
        if let Some(t) = s.cpu_temp.filter(|_| run.tester.is_some()) {
            run.max_cpu = run.max_cpu.max(t);
        }
        if let Some(t) = s.gpu_temp.filter(|_| run.burn.is_some()) {
            run.max_gpu = run.max_gpu.max(t);
        }
        let errors = run.tester.as_ref().map(|t| t.ram.errors.load(Ordering::Relaxed)).unwrap_or(0);
        let gpu_gone = run.burn.as_mut().map(|b| !b.alive()).unwrap_or(false);
        let gpu_outcome = run.burn.as_ref().and_then(|b| b.outcome.clone());
        let elapsed = run.start.elapsed();
        if let Some(reason) = check(&run.cfg, run.tester.is_some(), run.burn.is_some(), &s, errors, gpu_gone.then_some(gpu_outcome).flatten(), elapsed) {
            self.stop(&reason);
        }
    }

    pub fn status(&self) -> Status {
        let run = self.run.as_ref();
        let live = run.and_then(|r| r.burn.as_ref()).map(|b| b.live.lock().unwrap());
        let ram = run.and_then(|r| r.tester.as_ref()).map(|t| &t.ram);
        let fin = self.done.clone().unwrap_or_default();
        let ram_on = ram.map(|r| r.allocated.load(Ordering::Relaxed) > 0).unwrap_or(fin.ram_passes.is_some());
        Status {
            running: run.is_some(),
            status: self.status.0.clone(),
            alert: self.status.1,
            elapsed_s: run.map(|r| r.start.elapsed().as_secs_f64()).unwrap_or(fin.elapsed_s),
            config: run.map(|r| r.cfg.clone()).unwrap_or_else(|| self.cfg.clone()),
            sample: self.last.clone(),
            cpu_max: run.map(|r| (r.max_cpu > 0.0).then_some(r.max_cpu)).unwrap_or(fin.max_cpu),
            gpu_max: run.map(|r| (r.max_gpu > 0.0).then_some(r.max_gpu)).unwrap_or(fin.max_gpu),
            gpu_on: run.map(|r| r.burn.is_some()).unwrap_or(false),
            gpu_fps: live.as_ref().and_then(|l| l.fps),
            gpu_fps_avg: run.and_then(|r| r.burn.as_ref()).and_then(|b| b.avg_fps()).or(fin.gpu_fps_avg),
            gpu_adapter: live.as_ref().and_then(|l| l.adapter.clone()),
            ram_on,
            ram_allocated: ram.map(|r| r.allocated.load(Ordering::Relaxed)).unwrap_or(0),
            ram_passes: ram.map(|r| r.passes.load(Ordering::Relaxed)).or(fin.ram_passes).unwrap_or(0),
            ram_errors: ram.map(|r| r.errors.load(Ordering::Relaxed)).or(fin.ram_errors).unwrap_or(0),
            history_cpu: self.hist_cpu.iter().copied().collect(),
            history_gpu: self.hist_gpu.iter().copied().collect(),
            history_len: HISTORY_LEN,
            log: self.log.clone(),
        }
    }

    fn add_log(&mut self, msg: String) {
        self.log.push(format!("{}  {msg}", clock()));
        if self.log.len() > 500 {
            self.log.remove(0);
        }
    }
}

/// Decide se o teste para agora, na mesma ordem do Windows. `gpu_ended`: como o reset-gpu terminou, se terminou.
pub fn check(cfg: &Config, cpu_ram: bool, gpu: bool, s: &Sample, mem_errors: u64, gpu_ended: Option<Result<Option<GpuResult>, String>>, elapsed: Duration) -> Option<String> {
    if let Some(t) = s.cpu_temp.filter(|&t| cpu_ram && t >= cfg.cpu_limit) {
        return Some(format!("ABORTADO: CPU a {} atingiu o limite de {} °C", fmt_temp(Some(t)), fmt0(cfg.cpu_limit)));
    }
    if let Some(t) = s.gpu_temp.filter(|&t| gpu && t >= cfg.gpu_limit) {
        return Some(format!("ABORTADO: GPU a {} atingiu o limite de {} °C", fmt_temp(Some(t)), fmt0(cfg.gpu_limit)));
    }
    if mem_errors > 0 {
        return Some(format!("ABORTADO: {mem_errors} erros de memória detectados"));
    }
    if let Some(end) = gpu_ended {
        return Some(match end {
            Err(e) => format!("ABORTADO: falha na GPU ({e})"),
            Ok(Some(r)) if r.stopped.as_deref() == Some("temperatura") => {
                format!("ABORTADO: GPU atingiu o limite de {} °C (reset-gpu)", fmt0(cfg.gpu_limit))
            }
            Ok(Some(r)) if r.stopped.as_deref() == Some("tempo") => "Concluído (tempo esgotado)".into(),
            _ => "Janela da GPU fechada pelo usuário".into(),
        });
    }
    if cfg.minutes > 0.0 && elapsed.as_secs_f64() >= cfg.minutes * 60.0 {
        return Some("Concluído (tempo esgotado)".into());
    }
    None
}

fn push(h: &mut VecDeque<f64>, v: Option<f64>) {
    if let Some(v) = v {
        h.push_back(v);
        if h.len() > HISTORY_LEN {
            h.pop_front();
        }
    }
}

// ------------------------------------------------------------------ texto (padrão brasileiro)

fn fmt0(v: f64) -> String {
    format!("{v:.0}")
}

/// Até 2 casas, sem zeros sobrando, vírgula decimal: 0,5 · 10 · 1,25.
fn fmt2(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

/// "90,5 °C" (uma casa, sem ",0").
fn fmt_temp(v: Option<f64>) -> String {
    match v {
        Some(v) => {
            let s = format!("{v:.1}");
            format!("{} °C", s.strip_suffix(".0").unwrap_or(&s).replace('.', ","))
        }
        None => "—".into(),
    }
}

fn hms(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Hora local "HH:MM:SS".
fn clock() -> String {
    let t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padrao_da_ram_detecta_erro() {
        let mut c = vec![0u64; 4096];
        let p = pattern(3);
        fill(&mut c, p);
        assert_eq!(verify(&c, p), 0);
        assert_eq!(verify(&c, pattern(4)), 4096);
        c[100] ^= 1 << 17; // um bit "virado"
        c[4000] = 0;
        assert_eq!(verify(&c, p), 2);
    }

    #[test]
    fn worker_de_ram_pequeno() {
        let cancel = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(RamStats::default());
        let (c, s) = (cancel.clone(), stats.clone());
        let h = std::thread::spawn(move || ram_worker(&c, 4 << 20, 1 << 17, &s)); // 4 MB em blocos de 1 MB
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert_eq!(stats.allocated.load(Ordering::Relaxed), 4 << 20);
        assert!(stats.passes.load(Ordering::Relaxed) > 0);
        assert_eq!(stats.errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn carga_de_cpu_para_ao_cancelar() {
        let t = Tester::start(true, 0);
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        t.stop();
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    /// 4 s de carga de CPU lendo os sensores de verdade: `cargo test -p reset-core stress_curto -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn stress_curto() {
        let mut r = SampleReader::new();
        r.read();
        let t = Tester::start(true, 0);
        for _ in 0..4 {
            std::thread::sleep(Duration::from_secs(1));
            println!("{:?}", r.read());
        }
        t.stop();
    }

    #[test]
    fn limites() {
        let cfg = Config { cpu_limit: 90.0, gpu_limit: 85.0, minutes: 1.0, ..Config::default() };
        let s = |c: Option<f64>, g: Option<f64>| Sample { cpu_temp: c, gpu_temp: g, ..Sample::default() };
        let sec = Duration::from_secs;
        assert_eq!(check(&cfg, true, false, &s(Some(70.0), None), 0, None, sec(10)), None);
        assert_eq!(
            check(&cfg, true, false, &s(Some(90.5), None), 0, None, sec(10)).unwrap(),
            "ABORTADO: CPU a 90,5 °C atingiu o limite de 90 °C"
        );
        // GPU quente só conta se o stress de GPU estiver ligado
        assert_eq!(check(&cfg, true, false, &s(Some(60.0), Some(95.0)), 0, None, sec(10)), None);
        assert!(check(&cfg, false, true, &s(None, Some(85.0)), 0, None, sec(10)).unwrap().starts_with("ABORTADO: GPU a 85 °C"));
        assert_eq!(check(&cfg, true, false, &s(None, None), 3, None, sec(10)).unwrap(), "ABORTADO: 3 erros de memória detectados");
        assert_eq!(check(&cfg, false, true, &s(None, None), 0, Some(Ok(None)), sec(10)).unwrap(), "Janela da GPU fechada pelo usuário");
        assert!(check(&cfg, false, true, &s(None, None), 0, Some(Err("x".into())), sec(10)).unwrap().starts_with("ABORTADO: falha na GPU"));
        assert_eq!(check(&cfg, true, false, &s(Some(50.0), None), 0, None, sec(60)).unwrap(), "Concluído (tempo esgotado)");
        let forever = Config { minutes: 0.0, ..cfg };
        assert_eq!(check(&forever, true, false, &s(Some(50.0), None), 0, None, sec(99_999)), None);
    }

    #[test]
    fn validacao_e_textos() {
        assert!(Config { cpu: false, ram: false, gpu: false, ..Config::default() }.validate().is_err());
        assert!(Config { cpu_limit: 120.0, ..Config::default() }.validate().is_err());
        assert!(Config::default().validate().is_ok());
        assert_eq!(fmt2(0.5), "0,5");
        assert_eq!(fmt2(10.0), "10");
        assert_eq!(fmt_temp(Some(90.0)), "90 °C");
        assert_eq!(hms(3725.0), "01:02:05");
        assert_eq!(Config::default().gpu_desc(), "sim (1080p, MSAA 4x, Alta, limite 85 °C)");
    }

    #[test]
    fn sessao_registra_inicio_e_fim() {
        let mut st = Stress::new();
        st.start(Config { cpu: true, minutes: 0.0, ..Config::default() }).unwrap();
        assert!(st.running());
        assert!(st.start(Config::default()).is_err());
        st.tick(Sample { cpu_temp: Some(55.0), ..Sample::default() });
        st.stop("Parado pelo usuário");
        let s = st.status();
        assert!(!s.running);
        assert_eq!(s.status, "Parado");
        assert_eq!(s.log.len(), 2);
        assert!(s.log[1].contains("Parado pelo usuário · duração 00:00:00 · CPU temp. máx. 55 °C"), "{}", s.log[1]);
        assert_eq!(s.history_cpu, vec![55.0]);
    }
}
