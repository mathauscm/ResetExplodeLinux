//! Saída para o app: uma linha JSON por evento no stdout (contrato em linux/README.md, seção "reset-gpu").
//! Também a leitura de temperatura/consumo (NVML do reset-core, numa thread à parte para a consulta ao driver
//! não atrasar frames) e as contas de FPS médio e 1% low.

use crate::ctx::Adapter;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn emit(v: Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

pub fn info(a: &Adapter) {
    emit(json!({
        "type": "info",
        "adapter": a.name,
        "backend": "vulkan",
        "driver": a.driver,
        "ray_query": a.ray_query,
        "api": a.api,
        "vram": a.vram,
    }));
}

pub fn error(msg: &str) {
    emit(json!({ "type": "error", "message": msg }));
}

/// Arredonda para `d` casas (o JSON fica legível: 143.2 e não 143.18273).
pub fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (v * m).round() / m
}

/// Linha de progresso. `extra` entra no mesmo objeto (ex.: a fase do teste de ray tracing).
pub fn progress(elapsed: f64, fps: f64, s: Reading, extra: Value) {
    let mut v = json!({
        "type": "progress",
        "elapsed": round(elapsed, 1),
        "fps": round(fps, 1),
        "temp_c": s.temp_c.map(|t| t.round() as i64),
        "power_w": s.power_w.map(|p| round(p as f64, 1)),
    });
    merge(&mut v, extra);
    emit(v);
}

/// Linha de resultado. `stopped`: "tempo", "temperatura" ou "usuario".
pub fn result(test: &str, s: Summary, seconds: f64, score: Option<f64>, stopped: &str, extra: Value) {
    let mut v = json!({
        "type": "result",
        "test": test,
        "avg_fps": round(s.avg_fps, 1),
        "low1_fps": round(s.low1_fps, 1),
        "frames": s.frames,
        "seconds": round(seconds, 1),
        "score": score.map(|x| x.round() as i64),
        "stopped": stopped,
    });
    merge(&mut v, extra);
    emit(v);
}

pub fn merge(v: &mut Value, extra: Value) {
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Reading {
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub max_temp_c: Option<f32>,
}

/// Sensores da GPU lidos a cada 0,5 s numa thread própria. Sem NVIDIA/NVML, tudo fica null.
pub struct Sensors {
    shared: Arc<Mutex<Reading>>,
    stop: Arc<AtomicBool>,
}

impl Sensors {
    pub fn start(adapter: &str) -> Sensors {
        let shared = Arc::new(Mutex::new(Reading::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, st, name) = (shared.clone(), stop.clone(), adapter.to_string());
        thread::spawn(move || {
            if !reset_core::gpu::nvidia_available() {
                return;
            }
            // A placa do NVML com o mesmo nome da que o Vulkan escolheu (normalmente a 0).
            let index = reset_core::gpu::names().iter().find(|(_, n)| *n == name).map(|(i, _)| *i).unwrap_or(0);
            while !st.load(Ordering::Relaxed) {
                if let Some(l) = reset_core::gpu::live(index) {
                    let mut r = sh.lock().unwrap();
                    r.temp_c = l.temp_c;
                    r.power_w = l.power_w;
                    if let Some(t) = l.temp_c {
                        r.max_temp_c = Some(r.max_temp_c.map_or(t, |m| m.max(t)));
                    }
                }
                for _ in 0..10 {
                    if st.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        });
        Sensors { shared, stop }
    }

    pub fn read(&self) -> Reading {
        *self.shared.lock().unwrap()
    }
}

impl Drop for Sensors {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Tempos de frame (ms) e o FPS do último meio segundo, para o progresso.
#[derive(Default)]
pub struct Meter {
    pub times: Vec<f64>,
    last: f64,
    count: u32,
    pub fps: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Summary {
    pub avg_fps: f64,
    pub low1_fps: f64,
    pub frames: usize,
}

impl Meter {
    pub fn push(&mut self, ms: f64) {
        self.times.push(ms);
        self.count += 1;
    }

    /// Verdadeiro a cada ~0,5 s (`now` em segundos): hora de mandar o progresso.
    pub fn tick(&mut self, now: f64) -> bool {
        if now - self.last < 0.5 {
            return false;
        }
        self.fps = self.count as f64 / (now - self.last);
        self.count = 0;
        self.last = now;
        true
    }

    pub fn summary(&self) -> Summary {
        summarize(&self.times)
    }
}

/// FPS médio = frames ÷ soma dos tempos; 1% low = 1000 ÷ percentil 99 do tempo de frame (como no app do Windows).
pub fn summarize(times: &[f64]) -> Summary {
    if times.is_empty() {
        return Summary::default();
    }
    let mut sorted = times.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p99 = sorted[((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1)];
    let secs = times.iter().sum::<f64>() / 1000.0;
    Summary {
        avg_fps: if secs > 0.0 { times.len() as f64 / secs } else { 0.0 },
        low1_fps: if p99 > 0.0 { 1000.0 / p99 } else { 0.0 },
        frames: times.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumo() {
        let mut t = vec![10.0; 99];
        t.push(40.0);
        let s = summarize(&t);
        assert_eq!(s.frames, 100);
        assert!((s.avg_fps - 100.0 / 1.03).abs() < 1e-9);
        assert!((s.low1_fps - 25.0).abs() < 1e-9);
        assert_eq!(summarize(&[]).frames, 0);
    }
}
