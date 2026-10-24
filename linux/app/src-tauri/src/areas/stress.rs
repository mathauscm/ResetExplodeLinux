//! Área "stress" (Stress test). O teste e os limites de segurança ficam no backend (`reset_core::stress`):
//! uma thread lê os sensores a cada segundo e para tudo ao passar do limite de temperatura, mesmo com a
//! tela fechada. A interface só consulta o estado (`status`, 1x/s) e manda iniciar/parar.

use reset_core::stress::{Config, SampleReader, Stress};
use serde_json::{json, Value};
use std::sync::{LazyLock, Mutex, Once};
use std::time::{Duration, Instant};
use tauri::AppHandle;

static STRESS: LazyLock<Mutex<Stress>> = LazyLock::new(|| Mutex::new(Stress::new()));
/// Última vez que a tela pediu o estado: sem teste rodando, os sensores só são lidos com a tela aberta.
static WATCHED: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));
static MONITOR: Once = Once::new();

/// Stress test rodando (o benchmark consulta antes de começar).
pub fn running() -> bool {
    STRESS.lock().unwrap().running()
}

pub async fn dispatch(_app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    start_monitor();
    match method {
        "status" => {
            *WATCHED.lock().unwrap() = Instant::now();
            let s = STRESS.lock().unwrap().status();
            Ok(json!({
                "state": s,
                "gpu_tool": reset_core::bench::gpu::find().is_some(),
                "ram_available": reset_core::system::memory().available,
                "bench_running": super::bench::busy(),
            }))
        }
        "start" => {
            let cfg: Config = serde_json::from_value(args).map_err(|e| format!("configuração inválida: {e}"))?;
            if super::bench::busy() {
                return Err("Termine o benchmark antes de começar o stress test.".into());
            }
            // começar envolve alocar threads e abrir o reset-gpu: fora da thread assíncrona
            tauri::async_runtime::spawn_blocking(move || STRESS.lock().unwrap().start(cfg))
                .await
                .map_err(|e| e.to_string())??;
            Ok(Value::Null)
        }
        "stop" => {
            tauri::async_runtime::spawn_blocking(|| STRESS.lock().unwrap().stop("Parado pelo usuário")).await.map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        _ => Err(format!("stress: método desconhecido: {method}")),
    }
}

/// Thread única que lê os sensores 1x/s enquanto há teste rodando ou a tela está aberta, e aplica os limites.
fn start_monitor() {
    MONITOR.call_once(|| {
        let _ = std::thread::Builder::new().name("stress monitor".into()).spawn(|| {
            let mut reader = SampleReader::new();
            loop {
                let t0 = Instant::now();
                let watched = WATCHED.lock().unwrap().elapsed() < Duration::from_secs(5);
                if watched || running() {
                    let sample = reader.read(); // fora da trava: ler sensores leva alguns milissegundos
                    STRESS.lock().unwrap().tick(sample);
                }
                std::thread::sleep(Duration::from_secs(1).saturating_sub(t0.elapsed()));
            }
        });
    });
}
