//! RESET JOGOS: biblioteca de jogos instalados, FPS estimado e detector de gargalo.
//!
//! - `vdf`: leitor do formato de texto da Steam
//! - `library`: jogos da Steam, Heroic e Lutris, capas e "jogar" (só leitura nos arquivos das lojas)
//! - `launch`: perfil por jogo → linha de "Opções de inicialização"
//! - `catalog` + `specs`: catálogo de 32 jogos, índices de GPU/CPU e especificações de GPU
//! - `howmanyfps`: dados reais da API da HowManyFPS (com chave), com cache de 7 dias
//! - `bottleneck`: o que está limitando o FPS, ao vivo e numa sessão

pub mod bottleneck;
pub mod catalog;
pub mod howmanyfps;
pub mod launch;
pub mod library;
pub mod specs;
pub mod vdf;

use serde::Serialize;
use std::path::PathBuf;

/// ~/.config/reset-explode
pub(crate) fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("reset-explode"))
}

/// ~/.cache/reset-explode
pub(crate) fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("reset-explode"))
}

/// A GPU e a CPU deste PC, com os índices de desempenho usados nas estimativas.
#[derive(Debug, Clone, Serialize)]
pub struct Machine {
    pub cpu_name: String,
    pub gpu_name: String,
    pub cpu_short: String,
    pub gpu_short: String,
    /// RTX 4070 = 100
    pub gpu_idx: f64,
    /// Ryzen 7 7700X = 100
    pub cpu_idx: f64,
    /// false = o modelo não está na tabela e o índice é um palpite
    pub gpu_known: bool,
    pub cpu_known: bool,
    pub vram_total: Option<u64>,
    pub gpu_spec: Option<specs::GpuSpec>,
    pub gpu_features: Option<&'static str>,
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).trim().to_string())))
        .unwrap_or_default()
}

pub fn machine() -> Machine {
    let cpu_name = cpu_model();
    let gpu = crate::gpu::list().into_iter().next();
    let gpu_name = gpu.as_ref().map(|g| g.name.clone()).unwrap_or_default();
    let gpu_perf = catalog::gpu_perf(&gpu_name);
    let cpu_perf = catalog::cpu_perf(&cpu_name);
    let spec = gpu.as_ref().and_then(|g| specs::find(g.pci_device_id.as_deref()?, &g.name));
    Machine {
        cpu_short: catalog::short_cpu_name(&cpu_name),
        gpu_short: if gpu_name.is_empty() { "GPU não identificada".into() } else { catalog::short_gpu_name(&gpu_name) },
        gpu_idx: gpu_perf.unwrap_or(50.0),
        cpu_idx: cpu_perf.unwrap_or(80.0),
        gpu_known: gpu_perf.is_some(),
        cpu_known: cpu_perf.is_some(),
        vram_total: gpu.as_ref().and_then(|g| g.vram_total),
        gpu_features: spec.and_then(|s| specs::arch_features(s.arch)),
        gpu_spec: spec,
        cpu_name,
        gpu_name,
    }
}

/// Um cartão da tela "FPS estimado".
#[derive(Debug, Clone, Serialize)]
pub struct FpsRow {
    pub name: &'static str,
    pub genre: &'static str,
    pub fps: f64,
    pub low: f64,
    /// "ESTIMATIVA" ou "HOWMANYFPS"
    pub source: &'static str,
    /// gargalo, trava do jogo, erro da API...
    pub detail: String,
    pub limiter: Option<catalog::Limit>,
    pub rating: &'static str,
    /// O jogo está instalado neste PC (comparando o nome com a biblioteca).
    pub installed: bool,
}

pub fn estimate_all(m: &Machine, res: &str, preset: &str, up: &str, installed: &[String]) -> Vec<FpsRow> {
    let inst: Vec<String> = installed.iter().map(|n| catalog::norm_name(n)).collect();
    catalog::GAMES
        .iter()
        .map(|g| {
            let e = catalog::estimate(g, m.gpu_idx, m.cpu_idx, res, preset, up);
            let detail = match e.limiter {
                catalog::Limit::Cpu => "gargalo: CPU".to_string(),
                catalog::Limit::Gpu => "gargalo: GPU".to_string(),
                catalog::Limit::Engine => format!("trava do jogo em {:.0} FPS", g.engine_cap),
            };
            FpsRow {
                name: g.name,
                genre: g.genre,
                fps: e.avg,
                low: 0.0,
                source: "ESTIMATIVA",
                detail,
                limiter: Some(e.limiter),
                rating: catalog::rating(e.avg),
                installed: inst.contains(&catalog::norm_name(g.name)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_mark_installed_games() {
        let m = Machine {
            cpu_name: "AMD Ryzen 7 5700X 8-Core Processor".into(),
            gpu_name: "NVIDIA GeForce RTX 4060".into(),
            cpu_short: "Ryzen 7 5700X".into(),
            gpu_short: "RTX 4060".into(),
            gpu_idx: 70.0,
            cpu_idx: 77.0,
            gpu_known: true,
            cpu_known: true,
            vram_total: None,
            gpu_spec: None,
            gpu_features: None,
        };
        let rows = estimate_all(&m, "1920x1080", "Alto", "Native", &["Red Dead Redemption 2".into(), "Black Myth: Wukong".into()]);
        assert_eq!(rows.len(), 32);
        let inst: Vec<&str> = rows.iter().filter(|r| r.installed).map(|r| r.name).collect();
        assert_eq!(inst, ["Red Dead Redemption 2", "Black Myth: Wukong"]);
        let er = rows.iter().find(|r| r.name == "Elden Ring").unwrap();
        assert_eq!((er.fps, er.detail.as_str()), (60.0, "trava do jogo em 60 FPS")); // a RTX 4060 daria 77
    }
}
