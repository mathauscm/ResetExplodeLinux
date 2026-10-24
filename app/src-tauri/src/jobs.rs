//! Trabalhos longos com a saída ao vivo na interface: pacman, ajustes de sistema, etc. Com `root`, rodam
//! via `pkexec` (janela de senha do sistema). A interface recebe os eventos "job-log" (uma linha) e
//! "job-done" ({ok, message}); `app/ui/js/pkgjob.js` cuida disso.
//!
//! Quem chama monta o comando a partir de valores validados: nunca repassa texto livre da interface.

use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// Um trabalho por vez (dois pacman/pkexec juntos só dariam erro de trava).
pub struct JobLock(pub Mutex<bool>);

#[derive(Clone, Serialize)]
struct JobDone {
    ok: bool,
    message: String,
}

/// Roda `program args...` (caminho absoluto), com root se `root`, mandando cada linha para a interface.
/// Volta assim que o processo começa; o fim chega pelo evento "job-done".
pub fn spawn(app: &AppHandle, root: bool, program: &str, args: &[String]) -> Result<(), String> {
    let lock = app.state::<JobLock>();
    {
        let mut running = lock.0.lock().unwrap();
        if *running {
            return Err("já tem uma tarefa em andamento. Espere ela terminar.".into());
        }
        *running = true;
    }
    let mut cmd = if root {
        let mut c = Command::new("pkexec");
        c.arg(program);
        c
    } else {
        Command::new(program)
    };
    let child = cmd.args(args).env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            *lock.0.lock().unwrap() = false;
            return Err(if root { format!("não deu para pedir permissão de administrador (pkexec): {e}") } else { format!("não deu para rodar {program}: {e}") });
        }
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    let _ = app.emit("job-log", format!("$ {name} {}", args.join(" ")));
    let pipe = |r: Box<dyn Read + Send>, app: AppHandle| {
        std::thread::spawn(move || {
            let mut r = BufReader::new(r);
            let mut buf = Vec::new();
            while r.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                if !line.is_empty() {
                    let _ = app.emit("job-log", line);
                }
                buf.clear();
            }
        })
    };
    let out = pipe(Box::new(child.stdout.take().unwrap()), app.clone());
    let err = pipe(Box::new(child.stderr.take().unwrap()), app.clone());
    let app = app.clone();
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = (out.join(), err.join());
        let code = status.ok().and_then(|s| s.code());
        let (ok, message) = match code {
            Some(0) => (true, "Concluído.".to_string()),
            Some(126) if root => (false, "Cancelado: a senha não foi confirmada.".to_string()),
            Some(127) if root => (false, "Sem permissão de administrador para esta ação.".to_string()),
            Some(c) => (false, format!("Terminou com erro (código {c}). Veja o registro acima.")),
            None => (false, "O processo foi interrompido.".to_string()),
        };
        *app.state::<JobLock>().0.lock().unwrap() = false;
        let _ = app.emit("job-done", JobDone { ok, message });
    });
    Ok(())
}
