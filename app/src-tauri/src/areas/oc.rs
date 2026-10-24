//! Área "oc": overclock da GPU NVIDIA (NVML), CPU (só leitura), erros de hardware e verificador de estabilidade.
//!
//! Métodos: `info`, `gpu`, `apply {index, power_w, core_offset, mem_offset, clock_cap}`, `reset {index}`,
//! `errors`, `check_start {target, minutes}`, `check_status`, `check_stop`.
//!
//! Aplicar roda `pkexec <este executável> --nvml-apply ...` pelo `jobs::spawn` (senha na janela do sistema,
//! saída ao vivo na tela); o modo `--nvml-apply` valida tudo de novo com os limites lidos na hora.

use reset_core::oc::{self, CheckOptions, CheckProgress, CheckResult, OcRequest, Target};
use reset_core::system;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;

#[derive(Default, Clone, Serialize)]
struct CheckState {
    running: bool,
    progress: Option<CheckProgress>,
    result: Option<CheckResult>,
    #[serde(skip)]
    cancel: Option<Arc<AtomicBool>>,
}

fn check() -> &'static Mutex<CheckState> {
    static S: OnceLock<Mutex<CheckState>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// `reset-gpu` fica na mesma pasta do executável (ver linux/README.md).
fn reset_gpu_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join("reset-gpu");
    p.exists().then_some(p)
}

/// O `reset-gpu` responde ao `info` (a versão que só diz "não implementado" sai com erro). Espera até 5 s.
fn reset_gpu_works(p: &PathBuf) -> bool {
    let Ok(mut child) = std::process::Command::new(p)
        .arg("info")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        if let Ok(Some(s)) = child.try_wait() {
            return s.success();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

fn gpu(index: u32) -> Result<oc::GpuOc, String> {
    oc::gpus().into_iter().find(|g| g.index == index).ok_or_else(|| "GPU NVIDIA não encontrada pelo NVML".to_string())
}

fn index(args: &Value) -> u32 {
    args["index"].as_u64().unwrap_or(0) as u32
}

/// Roda o próprio executável com root no modo `--nvml-apply`.
fn spawn_apply(app: &AppHandle, req: &OcRequest) -> Result<Value, String> {
    let g = gpu(req.index)?;
    let plan = oc::validate(req, &g.limits)?;
    let exe = reset_core::util::self_exe().map_err(|e| format!("não deu para achar o executável do app: {e}"))?;
    crate::jobs::spawn(app, true, &exe.to_string_lossy(), &plan.args())?;
    Ok(Value::Null)
}

pub async fn dispatch(app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    match method {
        "info" => tauri::async_runtime::spawn_blocking(|| {
            let s = system::collect();
            let rg = reset_gpu_path();
            json!({
                "gpus": oc::gpus(),
                "cpu": oc::cpu_info(),
                "cpu_model": s.cpu.model,
                "cpu_vendor": s.cpu.vendor,
                "board": s.board,
                "gpu_load": rg.as_ref().is_some_and(reset_gpu_works),
                "safe": { "core": oc::CORE_OFFSET_SAFE, "mem": oc::MEM_OFFSET_SAFE },
                "limits_c": { "cpu": oc::CPU_LIMIT_C, "gpu": oc::GPU_LIMIT_C },
            })
        })
        .await
        .map_err(|e| e.to_string()),
        "gpu" => Ok(json!(gpu(index(&args))?)),
        "apply" => {
            let req: OcRequest = serde_json::from_value(args).map_err(|e| format!("pedido inválido: {e}"))?;
            spawn_apply(app, &req)
        }
        "reset" => {
            let i = index(&args);
            spawn_apply(app, &oc::reset_request(i, &gpu(i)?.limits))
        }
        "errors" => {
            let since = oc::now_s().saturating_sub(7 * 86_400);
            tauri::async_runtime::spawn_blocking(move || json!(oc::kernel_errors_since(since))).await.map_err(|e| e.to_string())
        }
        "check_start" => {
            let target: Target = serde_json::from_value(args["target"].clone()).map_err(|_| "alvo inválido")?;
            let minutes = args["minutes"].as_u64().filter(|m| (1..=60).contains(m)).ok_or("duração inválida")?;
            let opts = CheckOptions { target, seconds: minutes as u32 * 60, gpu_index: index(&args), reset_gpu: reset_gpu_path() };
            let cancel = Arc::new(AtomicBool::new(false));
            {
                let mut st = check().lock().unwrap();
                if st.running {
                    return Err("o verificador já está rodando.".into());
                }
                *st = CheckState { running: true, cancel: Some(cancel.clone()), ..Default::default() };
            }
            std::thread::spawn(move || {
                let r = oc::run_check(&opts, &cancel, |p| check().lock().unwrap().progress = Some(p.clone()));
                let mut st = check().lock().unwrap();
                st.running = false;
                st.result = Some(r);
                st.cancel = None;
            });
            Ok(Value::Null)
        }
        "check_status" => Ok(json!(check().lock().unwrap().clone())),
        "check_stop" => {
            if let Some(c) = &check().lock().unwrap().cancel {
                c.store(true, Ordering::Relaxed);
            }
            Ok(Value::Null)
        }
        _ => Err(format!("oc: método desconhecido: {method}")),
    }
}
