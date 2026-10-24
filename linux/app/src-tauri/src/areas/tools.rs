//! Área "tools": Otimização (ajustes e limpezas) e Inicialização (o que abre com o sistema).
//! A lógica fica em `reset_core::tweaks` e `reset_core::startup`; aqui só se escolhe o método e se roda o
//! que precisa de root pelo `jobs::spawn` (a interface acompanha pelos eventos "job-log"/"job-done").
//!
//! Da interface só vêm identificadores (id do ajuste, da limpeza ou do item); o core confere cada um contra o
//! estado lido na hora e monta o comando. Nada de texto livre vai para a linha de comando.

use crate::jobs;
use reset_core::startup::{self, Outcome, Paths};
use reset_core::tweaks::{self, Plan};
use serde_json::{json, Value};
use tauri::AppHandle;

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| format!("falta o parâmetro {key}"))
}

fn flag(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key).and_then(|v| v.as_bool()).ok_or_else(|| format!("falta o parâmetro {key}"))
}

/// Leituras demoradas (du, pacman, systemctl) fora da thread assíncrona.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

fn run_plan(app: &AppHandle, p: &Plan) -> Result<Value, String> {
    jobs::spawn(app, p.root, p.program, &p.args)?;
    Ok(json!({ "job": true, "title": p.title }))
}

pub async fn dispatch(app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    match method {
        // ---------------- Otimização
        "profile" => Ok(json!(blocking(tweaks::profile).await?)),
        "tweaks" => Ok(json!(blocking(tweaks::list).await?)),
        "cleanups" => Ok(json!(blocking(tweaks::cleanups).await?)),
        // {id, apply}: aplica (true) ou desfaz (false) um ajuste
        "tweak" => {
            let id = arg(&args, "id")?.to_string();
            let apply = flag(&args, "apply")?;
            let p = blocking(move || tweaks::plan(&id, apply)).await??;
            run_plan(app, &p)
        }
        // {id, key?}: limpa; com root vira tarefa, sem root é feito aqui e devolve quanto liberou
        "clean" => {
            let id = arg(&args, "id")?.to_string();
            let key = args.get("key").and_then(|v| v.as_str()).map(String::from);
            if ["pacman", "orphans", "journal"].contains(&id.as_str()) {
                let p = blocking(move || tweaks::clean_plan(&id)).await??;
                return run_plan(app, &p);
            }
            let freed = blocking(move || tweaks::clean_user(&id, key.as_deref())).await??;
            Ok(json!({ "job": false, "freed": freed }))
        }

        // ---------------- Inicialização
        "startup" => {
            let p = Paths::from_env()?;
            Ok(json!(blocking(move || startup::list(&p)).await?))
        }
        "boot" => Ok(json!(blocking(startup::boot).await?)),
        // {id, enabled}
        "startup_set" => {
            let p = Paths::from_env()?;
            let id = arg(&args, "id")?.to_string();
            let enabled = flag(&args, "enabled")?;
            match blocking(move || startup::set_enabled(&p, &id, enabled)).await?? {
                Outcome::Done(message) => Ok(json!({ "job": false, "message": message })),
                Outcome::Job(plan) => run_plan(app, &plan),
            }
        }
        // {id}: só atalhos do usuário ("autostart:<arquivo>")
        "startup_remove" => {
            let p = Paths::from_env()?;
            let id = arg(&args, "id")?;
            let file = id.strip_prefix("autostart:").ok_or("só dá para remover atalhos de autostart")?.to_string();
            let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
            let message = blocking(move || startup::remove_autostart(&p, &file, &desktop)).await??;
            Ok(json!({ "message": message }))
        }
        // {backup}
        "startup_restore" => {
            let p = Paths::from_env()?;
            let backup = arg(&args, "backup")?.to_string();
            let message = blocking(move || startup::restore_autostart(&p, &backup)).await??;
            Ok(json!({ "message": message }))
        }
        _ => Err(format!("tools: método desconhecido: {method}")),
    }
}
