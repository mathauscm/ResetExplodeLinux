//! Área "maker" (RESET MAKER): Portas seriais e Drivers seriais.
//!
//! Leitura: `ports` (portas, quem usa, permissão) e `drivers` (módulos, brltty, udev, Arduino IDE).
//! Monitor serial: `serial_open`/`serial_write`/`serial_reset`/`serial_close`; o que a placa manda chega pelo
//! evento "maker-serial" ({data}) e a queda da porta por "maker-serial-closed" ({reason}).
//! Correções com root (pkexec, saída ao vivo por `jobs`): `fix_group`, `udev_install`, `udev_remove`,
//! `brltty_mask`, `modprobe`. Instalar/remover pacotes usa o `pkg_run` de sempre (runJob no JS).
//!
//! Nada que vem da interface vai direto para a linha de comando: caminhos, grupos e módulos são conferidos
//! contra o que o próprio app leu do sistema.

use reset_core::maker::serial::{utf8_tail, SerialPort};
use reset_core::maker::{drivers, ports};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

struct Session {
    port: Arc<SerialPort>,
    path: String,
    baud: u32,
    stop: Arc<AtomicBool>,
}

/// Um monitor serial por vez.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

#[derive(Clone, Serialize)]
struct Chunk {
    data: String,
}

#[derive(Clone, Serialize)]
struct Closed {
    reason: String,
}

fn arg_str<'a>(args: &'a Value, k: &str) -> Result<&'a str, String> {
    args.get(k).and_then(Value::as_str).ok_or_else(|| format!("faltou o parâmetro {k}"))
}

fn to_json<T: Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

pub async fn dispatch(app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    match method {
        "ports" => to_json(blocking(ports::scan).await?),
        "drivers" => to_json(blocking(drivers::report).await?),

        // ------------------------------------------------ monitor serial
        "serial_state" => {
            let s = SESSION.lock().unwrap();
            Ok(match s.as_ref() {
                Some(s) => json!({ "open": true, "path": s.path, "baud": s.baud }),
                None => json!({ "open": false }),
            })
        }
        "serial_open" => {
            let path = arg_str(&args, "path")?.to_string();
            let baud = args.get("baud").and_then(Value::as_u64).ok_or("faltou a velocidade")? as u32;
            // só portas que o app mesmo listou
            let known = blocking(ports::scan).await?.ports.iter().any(|p| p.path == path);
            if !known {
                return Err(format!("{path} não está entre as portas seriais conectadas"));
            }
            close_session();
            let port = Arc::new(blocking({
                let path = path.clone();
                move || SerialPort::open(&path, baud)
            })
            .await??);
            let stop = Arc::new(AtomicBool::new(false));
            reader(app.clone(), port.clone(), stop.clone());
            *SESSION.lock().unwrap() = Some(Session { port, path: path.clone(), baud, stop });
            Ok(json!({ "open": true, "path": path, "baud": baud }))
        }
        "serial_write" => {
            let text = arg_str(&args, "text")?;
            let ending = match args.get("ending").and_then(Value::as_str).unwrap_or("lf") {
                "none" => "",
                "cr" => "\r",
                "crlf" => "\r\n",
                _ => "\n",
            };
            let port = current_port()?;
            let data = format!("{text}{ending}").into_bytes();
            blocking(move || port.write_all(&data)).await??;
            Ok(Value::Null)
        }
        "serial_reset" => {
            let port = current_port()?;
            blocking(move || port.pulse_reset()).await??;
            Ok(Value::Null)
        }
        "serial_close" => {
            close_session();
            Ok(json!({ "open": false }))
        }

        // ------------------------------------------------ correções
        "fix_group" => {
            let group = arg_str(&args, "group")?;
            let perm = blocking(ports::scan).await?.permission;
            if group != perm.group || !ports::valid_account_name(group) || !ports::valid_account_name(&perm.user) {
                return Err("grupo ou usuário inválido".into());
            }
            if perm.configured {
                return Err(format!("você já está no grupo {group}: falta sair da sessão e entrar de novo."));
            }
            crate::jobs::spawn(app, true, drivers::USERMOD, &["-aG".into(), group.into(), perm.user])?;
            Ok(Value::Null)
        }
        "kill_holder" => {
            let pid = args.get("pid").and_then(Value::as_u64).ok_or("faltou o processo")? as u32;
            let path = arg_str(&args, "path")?;
            if pid == std::process::id() || pid <= 1 || !path.starts_with("/dev/") || !ports::holds(pid, path) {
                return Err("esse processo não está mais usando a porta".into());
            }
            if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
                return Err(format!("não deu para encerrar: {}", std::io::Error::last_os_error()));
            }
            Ok(Value::Null)
        }
        "udev_install" => {
            let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let tmp = dir.join("reset-explode-70-reset-maker.rules");
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&tmp, drivers::rule_content()).map_err(|e| format!("não deu para preparar a regra: {e}"))?;
            crate::jobs::spawn(app, true, drivers::SH, &drivers::install_rule_args(&tmp))?;
            Ok(Value::Null)
        }
        "udev_remove" => {
            crate::jobs::spawn(app, true, drivers::SH, &drivers::remove_rule_args())?;
            Ok(Value::Null)
        }
        "brltty_mask" => {
            let unmask = args.get("unmask").and_then(Value::as_bool).unwrap_or(false);
            let b = blocking(drivers::report).await?.brltty;
            let names: Vec<String> = b.rules.iter().filter(|r| r.catches_ch340 && r.masked == unmask).map(|r| r.name.clone()).collect();
            crate::jobs::spawn(app, true, drivers::SH, &drivers::mask_args(&names, unmask)?)?;
            Ok(Value::Null)
        }
        "modprobe" => {
            let m = arg_str(&args, "module")?;
            if !drivers::valid_module(m) {
                return Err(format!("módulo desconhecido: {m}"));
            }
            crate::jobs::spawn(app, true, drivers::MODPROBE, &[m.to_string()])?;
            Ok(Value::Null)
        }
        "ide_flatpak" => {
            let scope = blocking(drivers::report).await?.ide.flathub.ok_or("o Flathub não está configurado no flatpak")?;
            crate::jobs::spawn(app, false, drivers::FLATPAK, &drivers::flatpak_ide_args(&scope)?)?;
            Ok(Value::Null)
        }
        _ => Err(format!("maker: método desconhecido: {method}")),
    }
}

fn current_port() -> Result<Arc<SerialPort>, String> {
    SESSION.lock().unwrap().as_ref().map(|s| s.port.clone()).ok_or_else(|| "o monitor serial está fechado".into())
}

fn close_session() {
    if let Some(s) = SESSION.lock().unwrap().take() {
        s.stop.store(true, Ordering::Relaxed);
        // a porta fecha quando a thread de leitura soltar a última referência (até 100 ms)
    }
}

/// Lê em segundo plano e manda para a interface em blocos (até ~40 ms ou 8 KB): a 2 Mbaud seriam
/// milhares de eventos por segundo.
fn reader(app: AppHandle, port: Arc<SerialPort>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut pending: Vec<u8> = Vec::new();
        let mut since: Option<Instant> = None;
        let flush = |pending: &mut Vec<u8>| {
            let keep = utf8_tail(pending);
            let cut = pending.len() - keep;
            if cut > 0 {
                let data = String::from_utf8_lossy(&pending[..cut]).into_owned();
                let _ = app.emit("maker-serial", Chunk { data });
                pending.drain(..cut);
            }
        };
        let reason = loop {
            if stop.load(Ordering::Relaxed) {
                break None;
            }
            let wait = if since.is_some() { 10 } else { 100 };
            match port.read_timeout(&mut buf, Duration::from_millis(wait)) {
                Ok(0) => {}
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    since.get_or_insert_with(Instant::now);
                }
                Err(e) => break Some(e),
            }
            if since.is_some_and(|t| t.elapsed() >= Duration::from_millis(40)) || pending.len() >= 8192 {
                flush(&mut pending);
                since = None;
            }
        };
        if !pending.is_empty() {
            let data = String::from_utf8_lossy(&pending).into_owned();
            let _ = app.emit("maker-serial", Chunk { data });
        }
        if let Some(reason) = reason {
            // a porta caiu: libera a sessão se ainda for esta
            let mut s = SESSION.lock().unwrap();
            if s.as_ref().is_some_and(|s| Arc::ptr_eq(&s.port, &port)) {
                *s = None;
            }
            drop(s);
            let _ = app.emit("maker-serial-closed", Closed { reason });
        }
    });
}
