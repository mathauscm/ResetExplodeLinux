//! Área "overlay": MangoHud (instalado?, MangoHud.conf gerado das escolhas, ligar em todos os jogos).
//!
//! Métodos: `state`, `preview {settings}`, `save {settings}`, `restore`, `global {on}`, `test`.

use reset_core::overlay::{self, OverlaySettings, Paths};
use serde_json::{json, Value};
use tauri::AppHandle;

fn paths() -> Result<Paths, String> {
    Paths::user().ok_or_else(|| "pasta de configuração do usuário não encontrada".to_string())
}

fn settings(args: &Value) -> Result<OverlaySettings, String> {
    serde_json::from_value(args["settings"].clone()).map_err(|e| format!("escolhas inválidas: {e}"))
}

fn state(p: &Paths) -> Value {
    let catalog: Vec<Value> = overlay::METRICS.iter().map(|m| json!(m)).collect();
    let toggles: Vec<Value> = overlay::TOGGLE_KEYS.iter().map(|(k, n)| json!({ "key": k, "label": n })).collect();
    json!({
        "catalog": catalog,
        "toggles": toggles,
        "presets": { "min": overlay::preset("min"), "default": overlay::preset("default"), "all": overlay::preset("all") },
        "conf": overlay::read_state(p),
        "paths": p,
        "global": overlay::global_state(p),
        "install": overlay::install_state(),
    })
}

pub async fn dispatch(_app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    match method {
        "state" => {
            let p = paths()?;
            tauri::async_runtime::spawn_blocking(move || state(&p)).await.map_err(|e| e.to_string())
        }
        // texto que seria gravado, para a tela mostrar antes de salvar
        "preview" => Ok(json!({ "text": overlay::generate(&settings(&args)?), "keys": overlay::mango_keys(&settings(&args)?.items) })),
        "save" => {
            let p = paths()?;
            let r = overlay::save(&p, &settings(&args)?)?;
            Ok(json!({ "backup": r.backup, "conf": overlay::read_state(&p) }))
        }
        "restore" => {
            let p = paths()?;
            let message = overlay::restore(&p)?;
            Ok(json!({ "message": message, "conf": overlay::read_state(&p) }))
        }
        "global" => {
            let p = paths()?;
            let on = args["on"].as_bool().ok_or("faltou {on}")?;
            overlay::set_global(&p, on)?;
            Ok(json!(overlay::global_state(&p)))
        }
        "test" => overlay::launch_test().map(|_| Value::Null),
        _ => Err(format!("overlay: método desconhecido: {method}")),
    }
}
