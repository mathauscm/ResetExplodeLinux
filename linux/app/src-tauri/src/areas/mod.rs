//! Telas novas: cada área tem o seu arquivo aqui e responde por um único comando, `area_call`.
//! A interface chama `call("area", "metodo", {args})` (app/ui/js/lib.js) e recebe JSON de volta.
//! Assim cada área cresce sem mexer no `main.rs` nem na lista de comandos do Tauri.
//!
//! Para trabalhos longos com saída ao vivo (com ou sem root), use `crate::jobs::spawn`.
//! Para avisar a interface no meio de algo demorado, use `app.emit("<area>-<evento>", payload)`.

pub mod bench;
pub mod energy;
pub mod games;
pub mod maker;
pub mod network;
pub mod oc;
pub mod overlay;
pub mod stress;
pub mod tools;

use serde_json::Value;
use tauri::AppHandle;

#[tauri::command]
pub async fn area_call(app: AppHandle, area: String, method: String, args: Option<Value>) -> Result<Value, String> {
    let args = args.unwrap_or(Value::Null);
    match area.as_str() {
        "bench" => bench::dispatch(&app, &method, args).await,
        "energy" => energy::dispatch(&app, &method, args).await,
        "games" => games::dispatch(&app, &method, args).await,
        "maker" => maker::dispatch(&app, &method, args).await,
        "network" => network::dispatch(&app, &method, args).await,
        "oc" => oc::dispatch(&app, &method, args).await,
        "overlay" => overlay::dispatch(&app, &method, args).await,
        "stress" => stress::dispatch(&app, &method, args).await,
        "tools" => tools::dispatch(&app, &method, args).await,
        _ => Err(format!("área desconhecida: {area}")),
    }
}
