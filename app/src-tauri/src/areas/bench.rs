//! Área "bench" (RESET BENCH): render de CPU, simulação de jogo, memória, disco e os testes de GPU pelo
//! `reset-gpu`. Um teste por vez, numa thread própria; a interface recebe o andamento por eventos:
//!
//! - `bench-render` (10x/s): blocos novos da imagem (tons de cinza em base64), blocos ativos e progresso
//! - `bench-sim` (10x/s): mapa da batalha, FPS e progresso
//! - `bench-progress`: fase e fração (memória e disco), com os resultados parciais
//! - `bench-gpu`: cada linha do reset-gpu
//! - `bench-done`: fim (resultado, cancelado ou erro) com o histórico atualizado

use reset_core::bench::{self, disk, gamesim, gpu, history::{self, History, Record, RenderRun}, memory, render};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Teste em andamento: (nome, cancelamento).
static JOB: Mutex<Option<(&'static str, Arc<AtomicBool>)>> = Mutex::new(None);

/// Algum teste do RESET BENCH rodando (o stress test consulta antes de começar).
pub fn busy() -> bool {
    JOB.lock().unwrap().is_some()
}

pub async fn dispatch(app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str, def: u64| args.get(k).and_then(Value::as_u64).unwrap_or(def);
    let b = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(false);
    match method {
        "info" => blocking(info).await,
        "cancel" => {
            if let Some((_, c)) = JOB.lock().unwrap().as_ref() {
                c.store(true, Ordering::Relaxed);
            }
            Ok(Value::Null)
        }
        "render_start" => {
            let mode = match s("mode").as_str() {
                "mt" => "mt",
                "st" => "st",
                _ => "both",
            };
            let sustained = b("sustained");
            start(app, "render", move |app, c| render_job(app, mode, sustained, c))
        }
        "sim_start" => {
            let secs = n("seconds", 20).clamp(5, 120);
            start(app, "sim", move |app, c| sim_job(app, secs, c))
        }
        "mem_start" => start(app, "mem", mem_job),
        "disk_drives" => blocking(|| Ok(json!(disk::drives()))).await,
        "disk_start" => {
            let mount = s("mount");
            let size = match n("size_mb", 1024) {
                512 => 512,
                4096 => 4096,
                _ => 1024,
            };
            let drive = disk::drives().into_iter().find(|d| d.mount == mount).ok_or("Escolha uma unidade.")?;
            start(app, "disk", move |app, c| disk_job(app, drive, size, c))
        }
        "gpu_info" => blocking(|| gpu::info().map(|i| json!(i))).await,
        "gpu_start" => {
            let (w, h) = (n("width", 1920) as u32, n("height", 1080) as u32);
            let full = b("fullscreen");
            let test = if s("test") == "rt" { "rt" } else { "gpu" };
            let args = if test == "rt" {
                gpu::rt_args(&s("preset"), w, h, n("seconds", 90) as u32, full)?
            } else {
                gpu::scene_args(w, h, n("seconds", 30) as u32, full)?
            };
            if gpu::find().is_none() {
                return Err(gpu::NOT_FOUND.into());
            }
            let preset = s("preset");
            start(app, test, move |app, c| gpu_job(app, test, args, preset, h, c))
        }
        _ => Err(format!("bench: método desconhecido: {method}")),
    }
}

async fn blocking(f: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn info() -> Result<Value, String> {
    let sys = reset_core::system::collect();
    Ok(json!({
        "cpu": sys.cpu.model,
        "cpu_short": bench::short_cpu_name(&sys.cpu.model),
        "cores": sys.cpu.cores,
        "threads": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(sys.cpu.threads),
        "debug": cfg!(debug_assertions),
        "gpu_tool": gpu::find().map(|p| p.display().to_string()),
        "running": JOB.lock().unwrap().as_ref().map(|j| j.0),
        "stress_running": super::stress::running(),
        "history": History::load(),
    }))
}

/// Reserva a vez e roda `job` numa thread; libera e avisa o fim no evento `bench-done`.
fn start<F>(app: &AppHandle, name: &'static str, job: F) -> Result<Value, String>
where
    F: FnOnce(&AppHandle, &AtomicBool) -> Result<Value, String> + Send + 'static,
{
    if super::stress::running() {
        return Err("Pare o stress test antes de rodar um benchmark.".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut j = JOB.lock().unwrap();
        if j.is_some() {
            return Err("Termine o outro teste antes.".into());
        }
        *j = Some((name, cancel.clone()));
    }
    let app = app.clone();
    std::thread::Builder::new()
        .name(format!("bench {name}"))
        .spawn(move || {
            let r = job(&app, &cancel);
            let cancelled = cancel.load(Ordering::Relaxed);
            *JOB.lock().unwrap() = None;
            let payload = match r {
                Ok(v) => json!({ "test": name, "ok": !cancelled, "cancelled": cancelled, "result": v, "history": History::load() }),
                Err(e) => json!({ "test": name, "ok": false, "cancelled": cancelled, "error": e, "history": History::load() }),
            };
            let _ = app.emit("bench-done", payload);
        })
        .map_err(|e| e.to_string())?;
    Ok(Value::Null)
}

fn save_record(test: &str, score: f64, summary: String) {
    let mut h = History::load();
    h.add_record(Record { date: history::now(), test: test.into(), score, summary });
    let _ = h.save();
}

// ------------------------------------------------------------------ render de CPU

fn render_job(app: &AppHandle, mode: &str, sustained: bool, cancel: &AtomicBool) -> Result<Value, String> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let phases: Vec<usize> = match mode {
        "mt" => vec![threads],
        "st" => vec![1],
        _ => vec![threads, 1],
    };
    let min = if sustained { Duration::from_secs(600) } else { Duration::ZERO };
    let started = Instant::now();
    let (mut multi, mut single) = (None, None);
    for th in phases {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let phase = if th > 1 { format!("Multi-core · {th} threads") } else { "Single-core · 1 thread".to_string() };
        let st = render::RenderState::new();
        let phase_start = Instant::now();
        let r = std::thread::scope(|s| {
            let h = s.spawn(|| st.run(th, min, cancel));
            let tick = |final_: bool| {
                let pass = st.pass().max(1);
                let in_pass = st.samples().saturating_sub((pass as u64 - 1) * render::SAMPLES_PER_PASS);
                let tiles: Vec<Value> =
                    st.take_tiles().into_iter().map(|t| json!({ "x": t.x, "y": t.y, "w": t.w, "h": t.h, "data": bench::b64(&t.data) })).collect();
                let active: Vec<[usize; 4]> = if final_ { Vec::new() } else { st.active().iter().map(|b| [b.x, b.y, b.w, b.h]).collect() };
                let _ = app.emit(
                    "bench-render",
                    json!({
                        "phase": phase, "mt": th > 1, "pass": pass, "frac": in_pass as f64 / render::SAMPLES_PER_PASS as f64,
                        "buckets_done": st.buckets_done(), "buckets": st.buckets.len(),
                        "elapsed": started.elapsed().as_secs_f64(), "phase_elapsed": phase_start.elapsed().as_secs_f64(),
                        "sustained": sustained, "active": active, "tiles": tiles,
                        "multi": multi, "single": single,
                    }),
                );
            };
            while !h.is_finished() {
                std::thread::sleep(Duration::from_millis(100));
                tick(false);
            }
            tick(true);
            h.join().map_err(|_| "o benchmark falhou".to_string())
        })?;
        if !cancel.load(Ordering::Relaxed) {
            if th > 1 {
                multi = Some(r.score);
            } else {
                single = Some(r.score);
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(Value::Null);
    }
    let cpu = bench::short_cpu_name(&reset_core::system::collect().cpu.model);
    let mut h = History::load();
    h.add_run(RenderRun { date: history::now(), multi, single, sustained, cpu });
    h.save()?;
    Ok(json!({ "multi": multi, "single": single, "threads": threads, "seconds": started.elapsed().as_secs_f64() }))
}

// ------------------------------------------------------------------ simulação de jogo

fn sim_job(app: &AppHandle, secs: u64, cancel: &AtomicBool) -> Result<Value, String> {
    let live = gamesim::SimLive::default();
    let mut sim = gamesim::GameSim::new();
    let r = std::thread::scope(|s| {
        let h = s.spawn(|| sim.run(Duration::from_secs(secs), cancel, &live));
        while !h.is_finished() {
            std::thread::sleep(Duration::from_millis(100));
            let _ = app.emit(
                "bench-sim",
                json!({
                    "warming": live.warming.load(Ordering::Relaxed), "progress": live.progress(), "fps": live.fps(),
                    "frames": live.frames.load(Ordering::Relaxed), "size": gamesim::VIEW, "view": bench::b64(&live.view()),
                }),
            );
        }
        h.join().map_err(|_| "a simulação falhou".to_string())
    })?;
    let Some(r) = r else { return Ok(Value::Null) };
    save_record("sim", r.score, format!("{} FPS · 1% low {}", bench::fmt_num(r.avg_fps, 0), bench::fmt_num(r.low1_fps, 0)));
    Ok(json!(r))
}

// ------------------------------------------------------------------ memória e carregamento

fn mem_job(app: &AppHandle, cancel: &AtomicBool) -> Result<Value, String> {
    let rows = Mutex::new(Vec::<Value>::new());
    let progress = |phase: &str, frac: f64| {
        let _ = app.emit("bench-progress", json!({ "test": "mem", "phase": phase, "frac": frac, "rows": *rows.lock().unwrap() }));
    };
    let row = |label: &str, value: String, hint: &str| rows.lock().unwrap().push(json!({ "label": label, "value": value, "hint": hint }));
    let stop = || cancel.load(Ordering::Relaxed);

    progress("Medindo latência da RAM…", 0.05);
    let lat = memory::latency_ns(cancel);
    if stop() {
        return Ok(Value::Null);
    }
    row("Latência da RAM", format!("{} ns", bench::fmt_num(lat, 1)), "Acesso aleatório. Menor é melhor; pesa muito em jogos competitivos.");
    progress("Medindo banda da RAM…", 0.2);
    let (read, copy) = memory::bandwidth(cancel);
    if stop() {
        return Ok(Value::Null);
    }
    row("Banda de leitura da RAM", format!("{} GB/s", bench::fmt_num(read, 1)), "Todas as threads lendo em sequência.");
    row("Banda de cópia da RAM", format!("{} GB/s", bench::fmt_num(copy, 1)), "Leitura + escrita.");
    progress("Preparando arquivos de teste", 0.35);

    let load = memory::loading(&memory::test_dir(), memory::LOAD_DEFAULT, &|p, f| progress(p, 0.35 + f * 0.6), cancel);
    let load = match load {
        Err(_) if stop() => return Ok(Value::Null),
        r => r?,
    };
    row(
        &format!("Disco ({}) · leitura sequencial", load.drive),
        format!("{} MB/s", bench::fmt_num(load.seq_read_mbs, 0)),
        "Pacote de 512 MB, sem o cache do sistema.",
    );
    row(
        "Assets pequenos",
        format!("{} arq/s", bench::fmt_num(load.files_per_sec, 0)),
        &format!("Mil arquivos de 64 KB com 8 threads ({} MB/s), como engines modernas.", bench::fmt_num(load.small_files_mbs, 0)),
    );
    row("Carregamento do mapa", format!("{} s", bench::fmt_num(load.map_load_seconds, 2)), "Pacote + assets: quanto menor, mais rápido o jogo carrega.");
    // Nota para a barra do histórico: mais rápido = maior (inverso da latência × velocidade de carga).
    save_record(
        "mem",
        1000.0 / lat + 10.0 / load.map_load_seconds,
        format!("{} ns · {} GB/s · mapa {} s", bench::fmt_num(lat, 0), bench::fmt_num(read, 0), bench::fmt_num(load.map_load_seconds, 2)),
    );
    let rows = rows.lock().unwrap().clone();
    Ok(json!({ "rows": rows }))
}

// ------------------------------------------------------------------ disco

fn disk_job(app: &AppHandle, drive: disk::Drive, size_mb: u64, cancel: &AtomicBool) -> Result<Value, String> {
    let label = drive.mount.clone();
    let progress = |phase: &str, frac: f64| {
        let _ = app.emit("bench-progress", json!({ "test": "disk", "phase": phase, "frac": frac }));
    };
    let r = match disk::run(&drive.dir, &label, disk::DiskOptions { file_mb: size_mb, random_secs: 4.0 }, &progress, cancel) {
        Err(_) if cancel.load(Ordering::Relaxed) => return Ok(Value::Null),
        r => r?,
    };
    let n0 = |v: f64| bench::fmt_num(v, 0);
    let rows = json!([
        { "label": "Leitura sequencial", "value": format!("{} MB/s", n0(r.seq_read_mbs)), "hint": "Blocos de 1 MB. Mede copiar arquivos grandes e carregar jogos." },
        { "label": "Escrita sequencial", "value": format!("{} MB/s", n0(r.seq_write_mbs)), "hint": "Blocos de 1 MB, direto no disco. SSDs com cache SLC caem depois de gravar dezenas de GB." },
        { "label": "Leitura 4 KB aleatória (1 por vez)", "value": format!("{} IOPS", n0(r.read_iops_q1)), "hint": format!("Latência média de {} µs. É o que define a sensação de rapidez do sistema e dos programas.", bench::fmt_num(r.read_latency_us, 1)) },
        { "label": "Escrita 4 KB aleatória (1 por vez)", "value": format!("{} IOPS", n0(r.write_iops_q1)), "hint": format!("Latência média de {} µs.", bench::fmt_num(r.write_latency_us, 1)) },
        { "label": "Leitura 4 KB aleatória (8 em paralelo)", "value": format!("{} IOPS", n0(r.read_iops_q8)), "hint": "Várias leituras ao mesmo tempo, como ao abrir muitos programas." },
    ]);
    save_record(
        "disk",
        r.score,
        format!("{label} · {}/{} MB/s · 4K {}k IOPS", n0(r.seq_read_mbs), n0(r.seq_write_mbs), bench::fmt_num(r.read_iops_q1 / 1000.0, 1)),
    );
    Ok(json!({ "rows": rows, "result": r }))
}

// ------------------------------------------------------------------ GPU (reset-gpu)

fn gpu_job(app: &AppHandle, test: &'static str, args: Vec<String>, preset: String, height: u32, cancel: &AtomicBool) -> Result<Value, String> {
    let r = gpu::run(&args, cancel, &mut |ev| {
        let _ = app.emit("bench-gpu", json!({ "test": test, "event": ev }));
    })?;
    let Some(r) = r else { return Ok(Value::Null) };
    if r.stopped.as_deref() == Some("usuario") {
        return Err("Janela fechada antes do fim: o resultado não vale como nota.".into());
    }
    let score = r.score.unwrap_or(0.0);
    let fps = r.avg_fps.unwrap_or(0.0);
    let pts = bench::fmt_num(score, 0);
    if test == "rt" {
        let name = match preset.as_str() {
            "normal" => "Normal",
            "extrema" => "Extrema",
            _ => "Alta",
        };
        save_record(&format!("rt:{preset}:{height}"), score, format!("{pts} pts · {} FPS · {name} {height}p", bench::fmt_num(fps, 0)));
    } else {
        save_record("gpu", score, format!("{pts} pts · {} FPS · {height}p", bench::fmt_num(fps, 0)));
    }
    Ok(json!(r))
}
