//! Testes de GPU pelo programa `reset-gpu` (Vulkan, janela própria): cena de jogo, ray tracing e stress.
//! O contrato está no README (seção "reset-gpu"): uma linha JSON por evento no stdout.
//!
//! O programa é procurado na pasta do executável atual (~/.local/bin depois do install-local.sh,
//! target/<perfil> no desenvolvimento) e depois no PATH.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const PROGRAM: &str = "reset-gpu";

/// Uma linha da saída do reset-gpu. Campos que faltarem viram `None` (versões diferentes continuam funcionando).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum GpuEvent {
    Info {
        #[serde(default)]
        adapter: Option<String>,
        #[serde(default)]
        backend: Option<String>,
        #[serde(default)]
        driver: Option<String>,
        #[serde(default)]
        ray_query: Option<bool>,
    },
    Progress {
        #[serde(default)]
        elapsed: Option<f64>,
        #[serde(default)]
        fps: Option<f64>,
        #[serde(default)]
        temp_c: Option<f64>,
        #[serde(default)]
        power_w: Option<f64>,
    },
    Result(GpuResult),
    Error {
        #[serde(default)]
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuResult {
    #[serde(default)]
    pub test: String,
    #[serde(default)]
    pub avg_fps: Option<f64>,
    #[serde(default)]
    pub low1_fps: Option<f64>,
    #[serde(default)]
    pub frames: Option<u64>,
    #[serde(default)]
    pub seconds: Option<f64>,
    #[serde(default)]
    pub score: Option<f64>,
    /// "tempo", "temperatura" ou "usuario".
    #[serde(default)]
    pub stopped: Option<String>,
}

/// Lê uma linha; o que não for JSON do contrato (mensagens do driver Vulkan, por exemplo) é ignorado.
pub fn parse_line(line: &str) -> Option<GpuEvent> {
    let l = line.trim();
    if !l.starts_with('{') {
        return None;
    }
    serde_json::from_str(l).ok()
}

/// Onde está o reset-gpu: ao lado do executável atual, senão no PATH.
pub fn find() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join(PROGRAM)));
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(PROGRAM)).collect::<Vec<_>>()).unwrap_or_default();
    beside.into_iter().chain(path).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

pub const NOT_FOUND: &str = "O programa reset-gpu não foi encontrado. Ele faz os testes de GPU numa janela própria (Vulkan) \
e vem junto com o RESET EXPLODE: rode ./install-local.sh de novo ou, no desenvolvimento, cargo build -p reset-gpu.";

// ------------------------------------------------------------------ parâmetros (validados: nada de texto livre)

const RESOLUTIONS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if RESOLUTIONS.contains(&(w, h)) { Ok(()) } else { Err(format!("resolução não suportada: {w}x{h}")) }
}

fn size_args(w: u32, h: u32, fullscreen: bool) -> Vec<String> {
    let mut a = vec!["--width".into(), w.to_string(), "--height".into(), h.to_string()];
    if fullscreen {
        a.push("--fullscreen".into());
    }
    a
}

pub fn scene_args(w: u32, h: u32, seconds: u32, fullscreen: bool) -> Result<Vec<String>, String> {
    check_size(w, h)?;
    let mut a = vec!["scene".to_string()];
    a.extend(size_args(w, h, fullscreen));
    a.extend(["--seconds".into(), seconds.clamp(5, 600).to_string()]);
    Ok(a)
}

pub fn rt_args(preset: &str, w: u32, h: u32, seconds: u32, fullscreen: bool) -> Result<Vec<String>, String> {
    check_size(w, h)?;
    if !["normal", "alta", "extrema"].contains(&preset) {
        return Err(format!("preset desconhecido: {preset}"));
    }
    let mut a = vec!["rt".to_string(), "--preset".into(), preset.into()];
    a.extend(size_args(w, h, fullscreen));
    a.extend(["--seconds".into(), seconds.clamp(5, 600).to_string()]);
    Ok(a)
}

/// `w`/`h` são ignorados em tela cheia. `seconds` 0 = sem limite (o app para quando quiser).
pub fn burn_args(w: u32, h: u32, fullscreen: bool, msaa: u32, intensity: &str, seconds: u64, temp_limit: u32) -> Result<Vec<String>, String> {
    if !fullscreen {
        check_size(w, h)?;
    }
    if ![0, 4, 8].contains(&msaa) {
        return Err(format!("MSAA inválido: {msaa}"));
    }
    if !["normal", "alta", "extrema"].contains(&intensity) {
        return Err(format!("intensidade desconhecida: {intensity}"));
    }
    let (w, h) = if fullscreen { (1920, 1080) } else { (w, h) };
    let mut a = vec!["burn".to_string()];
    a.extend(size_args(w, h, fullscreen));
    let secs = if seconds == 0 { 7 * 24 * 3600 } else { seconds.min(7 * 24 * 3600) };
    a.extend([
        "--msaa".into(),
        msaa.to_string(),
        "--intensity".into(),
        intensity.into(),
        "--seconds".into(),
        secs.to_string(),
        "--temp-limit".into(),
        temp_limit.clamp(40, 100).to_string(),
    ]);
    Ok(a)
}

// ------------------------------------------------------------------ execução

/// Roda o reset-gpu com `args`, chamando `on_event` a cada linha. Bloqueia até o programa terminar.
/// Com `cancel` = true o programa recebe SIGTERM (como fechar a janela). Devolve o resultado, se veio.
pub fn run(args: &[String], cancel: &AtomicBool, on_event: &mut dyn FnMut(&GpuEvent)) -> Result<Option<GpuResult>, String> {
    let exe = find().ok_or_else(|| NOT_FOUND.to_string())?;
    let mut cmd = Command::new(&exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Se o app fechar no meio, o reset-gpu fecha junto (o sinal vai quando ESTA thread terminar,
    // por isso quem chama espera o programa na mesma thread que o iniciou).
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("não deu para abrir o reset-gpu: {e}"))?;
    let pid = child.id() as i32;

    let stderr = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            s
        })
    });

    let mut result = None;
    let mut error = None;
    let finished = AtomicBool::new(false);
    let status = std::thread::scope(|s| {
        // vigia o cancelamento enquanto a leitura bloqueia no stdout; se o programa não fechar em 3 s, SIGKILL
        s.spawn(|| {
            let mut sent: Option<std::time::Instant> = None;
            while !finished.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    match sent {
                        None => {
                            unsafe { libc::kill(pid, libc::SIGTERM) };
                            sent = Some(std::time::Instant::now());
                        }
                        Some(t) if t.elapsed() > Duration::from_secs(3) => {
                            unsafe { libc::kill(pid, libc::SIGKILL) };
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if let Some(ev) = parse_line(&line) {
                    match &ev {
                        GpuEvent::Result(r) => result = Some(r.clone()),
                        GpuEvent::Error { message } => error = Some(message.clone()),
                        _ => {}
                    }
                    on_event(&ev);
                }
            }
        }
        let status = child.wait();
        finished.store(true, Ordering::Relaxed);
        status
    });
    let err_text = stderr.and_then(|h| h.join().ok()).unwrap_or_default();

    if result.is_some() {
        return Ok(result);
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if let Some(e) = error {
        return Err(e);
    }
    match status {
        Ok(s) if s.success() => Ok(None),
        Ok(s) => {
            let tail = err_text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            Err(match (s.code(), tail.is_empty()) {
                (Some(c), true) => format!("o reset-gpu terminou com erro (código {c})."),
                (Some(c), false) => format!("o reset-gpu terminou com erro (código {c}): {tail}"),
                (None, _) => "o reset-gpu foi interrompido.".into(),
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `reset-gpu info`: placa, backend, driver e se tem ray tracing por hardware (ray query).
/// Desiste depois de 15 s (driver travado não pode segurar a tela).
pub fn info() -> Result<GpuEvent, String> {
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let timer = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        timer.store(true, Ordering::Relaxed);
    });
    let mut info = None;
    let r = run(&["info".to_string()], &cancel, &mut |ev| {
        if matches!(ev, GpuEvent::Info { .. }) {
            info = Some(ev.clone());
        }
    });
    if info.is_none() && cancel.load(Ordering::Relaxed) {
        return Err("o reset-gpu não respondeu (15 s).".into());
    }
    match (info, r) {
        (Some(i), _) => Ok(i),
        (None, Err(e)) => Err(e),
        (None, Ok(_)) => Err("o reset-gpu não informou a placa de vídeo.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linhas_do_contrato() {
        assert_eq!(
            parse_line(r#"{"type":"info","adapter":"NVIDIA GeForce RTX 4060","backend":"vulkan","driver":"580","ray_query":true}"#),
            Some(GpuEvent::Info { adapter: Some("NVIDIA GeForce RTX 4060".into()), backend: Some("vulkan".into()), driver: Some("580".into()), ray_query: Some(true) })
        );
        assert_eq!(
            parse_line(r#"{"type":"progress","elapsed":12.5,"fps":143.2,"temp_c":61,"power_w":null}"#),
            Some(GpuEvent::Progress { elapsed: Some(12.5), fps: Some(143.2), temp_c: Some(61.0), power_w: None })
        );
        let r = parse_line(r#"{"type":"result","test":"scene","avg_fps":680.1,"low1_fps":512.3,"frames":20403,"seconds":30.0,"score":6801,"stopped":"tempo","extra":1}"#);
        let Some(GpuEvent::Result(r)) = r else { panic!("{r:?}") };
        assert_eq!((r.score, r.frames, r.stopped.as_deref()), (Some(6801.0), Some(20403), Some("tempo")));
        assert_eq!(parse_line(r#"{"type":"error","message":"sem Vulkan"}"#), Some(GpuEvent::Error { message: "sem Vulkan".into() }));
        assert_eq!(parse_line("MESA: aviso qualquer"), None);
        assert_eq!(parse_line(r#"{"type":"outra"}"#), None);
    }

    /// Um reset-gpu de mentira (script) no PATH: resultado, erro, cancelamento e o stress de GPU parando
    /// pela temperatura. Tudo num teste só, porque mexe no PATH do processo.
    #[test]
    fn reset_gpu_falso_no_path() {
        use crate::stress::{Config, Sample, Stress};
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("reset-gpu-falso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join(PROGRAM);
        let write = |body: &str| {
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let old = std::env::var_os("PATH").unwrap_or_default();
        std::env::set_var("PATH", std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&old))).unwrap());

        // resultado normal, com lixo no meio
        write(r#"echo '{"type":"info","adapter":"GPU de teste","backend":"vulkan","ray_query":false}'
echo 'aviso do driver'
echo '{"type":"progress","elapsed":0.5,"fps":100,"temp_c":50,"power_w":null}'
echo '{"type":"result","test":"scene","avg_fps":100.5,"low1_fps":90,"frames":3015,"seconds":30,"score":1005,"stopped":"tempo"}'"#);
        let cancel = AtomicBool::new(false);
        let mut n = 0;
        let r = run(&scene_args(1920, 1080, 30, false).unwrap(), &cancel, &mut |_| n += 1).unwrap().unwrap();
        assert_eq!((r.score, n), (Some(1005.0), 3));
        assert!(matches!(info().unwrap(), GpuEvent::Info { ray_query: Some(false), .. }));

        // erro do contrato e saída sem nada
        write(r#"echo '{"type":"error","message":"sem Vulkan"}'; exit 2"#);
        assert_eq!(run(&["info".into()], &cancel, &mut |_| {}).unwrap_err(), "sem Vulkan");
        write("echo falhou >&2; exit 3");
        assert_eq!(run(&["info".into()], &cancel, &mut |_| {}).unwrap_err(), "o reset-gpu terminou com erro (código 3): falhou");

        // cancelar encerra o programa (SIGTERM) em vez de esperar os 30 s
        write("exec sleep 30");
        let t = std::time::Instant::now();
        let c2 = std::sync::Arc::new(AtomicBool::new(false));
        let c3 = c2.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c3.store(true, Ordering::Relaxed);
        });
        assert_eq!(run(&["scene".into()], &c2, &mut |_| {}), Ok(None));
        assert!(t.elapsed() < Duration::from_secs(5));

        // stress de GPU: o reset-gpu parou pela temperatura → o stress test aborta e registra
        write(r#"echo '{"type":"progress","elapsed":1,"fps":200,"temp_c":86}'
echo '{"type":"result","test":"burn","avg_fps":200,"frames":400,"seconds":2,"stopped":"temperatura"}'"#);
        let mut st = Stress::new();
        st.start(Config { cpu: false, ram: false, gpu: true, minutes: 0.0, ..Config::default() }).unwrap();
        for _ in 0..50 {
            st.tick(Sample { gpu_temp: Some(70.0), ..Sample::default() });
            if !st.running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let s = st.status();
        assert!(!s.running && s.alert, "{s:?}");
        assert!(s.log[1].contains("ABORTADO: GPU atingiu o limite de 85 °C (reset-gpu)"), "{}", s.log[1]);
        assert!(s.log[1].contains("GPU: 400 frames, média 200 FPS"), "{}", s.log[1]);

        std::env::set_var("PATH", old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn argumentos_validados() {
        assert_eq!(scene_args(1920, 1080, 30, false).unwrap().join(" "), "scene --width 1920 --height 1080 --seconds 30");
        assert_eq!(rt_args("alta", 2560, 1440, 30, true).unwrap().join(" "), "rt --preset alta --width 2560 --height 1440 --fullscreen --seconds 30");
        assert!(rt_args("; rm -rf", 1920, 1080, 30, false).is_err());
        assert!(scene_args(123, 456, 30, false).is_err());
        assert_eq!(
            burn_args(1280, 720, false, 4, "alta", 600, 85).unwrap().join(" "),
            "burn --width 1280 --height 720 --msaa 4 --intensity alta --seconds 600 --temp-limit 85"
        );
        assert!(burn_args(1280, 720, false, 2, "alta", 600, 85).is_err());
        assert!(burn_args(0, 0, true, 0, "normal", 0, 85).unwrap().contains(&"--fullscreen".to_string()));
    }
}
