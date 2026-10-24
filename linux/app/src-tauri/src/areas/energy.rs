//! Área "energy": fonte/bateria, perfil de energia, política da CPU e consumo da GPU.
//!
//! - `overview` → tudo de uma vez (`energy::EnergyInfo`)
//! - `live` → consumo da GPU e da CPU e o clock de cada núcleo, para atualizar a cada 2 s
//! - `set_profile {id}` → troca o perfil do power-profiles-daemon (sem senha)
//! - `cpu_set {kind: "governor"|"epp"|"boost", value}` → muda a CPU com root (pkexec), saída pelos eventos de `jobs`

use crate::jobs;
use reset_core::energy::{self, CpuChange, CpuEnergyMeter};
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::AppHandle;

/// O consumo da CPU sai da diferença entre duas leituras do RAPL: o medidor fica vivo entre as chamadas.
static METER: Mutex<Option<CpuEnergyMeter>> = Mutex::new(None);

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

fn arg<'a>(args: &'a Value, k: &str) -> Result<&'a Value, String> {
    args.get(k).ok_or_else(|| format!("faltou o argumento \"{k}\""))
}

pub async fn dispatch(app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    match method {
        "overview" => blocking(|| json!(energy::overview())).await,
        "live" => {
            blocking(|| {
                let cpu = METER.lock().unwrap().get_or_insert_with(CpuEnergyMeter::default).poll();
                json!({ "gpus": energy::gpu_power(), "cpu": cpu, "clocks": energy::cpu_clocks() })
            })
            .await
        }
        "set_profile" => {
            let id = arg(&args, "id")?.as_str().ok_or("perfil inválido")?.to_string();
            blocking(move || energy::set_profile(&id)).await??;
            Ok(json!(energy::profiles()))
        }
        "cpu_set" => {
            let value = arg(&args, "value")?;
            let change = match arg(&args, "kind")?.as_str() {
                Some("governor") => CpuChange::Governor(value.as_str().ok_or("governor inválido")?.into()),
                Some("epp") => CpuChange::Epp(value.as_str().ok_or("EPP inválido")?.into()),
                Some("boost") => CpuChange::Boost(value.as_bool().ok_or("boost inválido")?),
                _ => return Err("mudança desconhecida".into()),
            };
            let (program, args) = energy::cpu_change(&change)?;
            jobs::spawn(app, true, &program, &args)?;
            Ok(Value::Null)
        }
        _ => Err(format!("energy: método desconhecido: {method}")),
    }
}
