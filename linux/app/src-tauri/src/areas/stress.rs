//! Área "stress" (a fazer).

use serde_json::Value;
use tauri::AppHandle;

pub async fn dispatch(_app: &AppHandle, method: &str, _args: Value) -> Result<Value, String> {
    Err(format!("stress: método desconhecido: {method}"))
}
