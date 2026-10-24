//! Área "network": adaptadores, tráfego e o diagnóstico de lag.
//!
//! - `overview` → adaptadores, gateway e DNS (`network::NetOverview`)
//! - `traffic` → contadores de bytes por interface (a interface calcula a taxa)
//! - `start` → começa o diagnóstico num thread; o andamento chega pelo evento "network-progress"
//!   (`network::TestEvent`, mais `{type:"done", report}`, `{type:"cancelled"}` ou `{type:"error", message}` no fim)
//! - `cancel` → interrompe o diagnóstico
//! - `status` → `{running, last}`: quem volta para a tela no meio do teste continua de onde estava

use reset_core::network::{self, NetReport};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Option<NetReport>> = Mutex::new(None);

pub async fn dispatch(app: &AppHandle, method: &str, _args: Value) -> Result<Value, String> {
    match method {
        "overview" => tauri::async_runtime::spawn_blocking(|| json!(network::overview())).await.map_err(|e| e.to_string()),
        "traffic" => Ok(json!(network::traffic())),
        "status" => Ok(json!({ "running": RUNNING.load(Ordering::SeqCst), "last": *LAST.lock().unwrap() })),
        "cancel" => {
            CANCEL.store(true, Ordering::SeqCst);
            Ok(Value::Null)
        }
        "start" => {
            if RUNNING.swap(true, Ordering::SeqCst) {
                return Err("o diagnóstico já está rodando".into());
            }
            CANCEL.store(false, Ordering::SeqCst);
            let app = app.clone();
            std::thread::spawn(move || {
                let emit = |e: network::TestEvent| {
                    let _ = app.emit("network-progress", e);
                };
                let end = match network::run_test(&CANCEL, &emit) {
                    _ if CANCEL.load(Ordering::SeqCst) => json!({ "type": "cancelled" }),
                    Ok(report) => {
                        *LAST.lock().unwrap() = Some(report.clone());
                        json!({ "type": "done", "report": report })
                    }
                    Err(message) => json!({ "type": "error", "message": message }),
                };
                RUNNING.store(false, Ordering::SeqCst);
                let _ = app.emit("network-progress", end);
            });
            Ok(Value::Null)
        }
        _ => Err(format!("network: método desconhecido: {method}")),
    }
}
