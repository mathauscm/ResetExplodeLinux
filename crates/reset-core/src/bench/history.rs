//! Histórico do RESET BENCH em ~/.config/reset-explode/bench.json (no Windows ficava no settings.json).
//! Render de CPU: últimas 10 execuções (multi e/ou single). Demais testes: últimos 10 de cada um.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP: usize = 10;

/// Uma execução do render de CPU.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RenderRun {
    /// Segundos desde 1970 (UTC).
    pub date: u64,
    pub multi: Option<f64>,
    pub single: Option<f64>,
    /// Passadas repetidas por no mínimo 10 min.
    pub sustained: bool,
    pub cpu: String,
}

/// Resultado dos outros testes ("sim", "mem", "disk", "gpu", "rt:alta:1080"...).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub date: u64,
    pub test: String,
    pub score: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct History {
    #[serde(default)]
    pub runs: Vec<RenderRun>,
    #[serde(default)]
    pub records: Vec<Record>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("reset-explode").join("bench.json"))
}

impl History {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = path().ok_or("sem pasta de configuração (HOME)")?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(tmp, p).map_err(|e| e.to_string())
    }

    pub fn add_run(&mut self, run: RenderRun) {
        self.runs.insert(0, run);
        self.runs.truncate(KEEP);
    }

    /// Mantém os 10 mais recentes de cada teste.
    pub fn add_record(&mut self, rec: Record) {
        self.records.insert(0, rec);
        let mut seen: Vec<(String, usize)> = Vec::new();
        self.records.retain(|r| match seen.iter_mut().find(|s| s.0 == r.test) {
            Some(s) => {
                s.1 += 1;
                s.1 <= KEEP
            }
            None => {
                seen.push((r.test.clone(), 1));
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarda_dez_de_cada() {
        let mut h = History::default();
        for i in 0..15 {
            h.add_run(RenderRun { date: i, multi: Some(i as f64), single: None, sustained: false, cpu: "x".into() });
            h.add_record(Record { date: i, test: "sim".into(), score: i as f64, summary: String::new() });
            h.add_record(Record { date: i, test: "disk".into(), score: i as f64, summary: String::new() });
        }
        h.add_record(Record { date: 99, test: "mem".into(), score: 1.0, summary: String::new() });
        assert_eq!(h.runs.len(), 10);
        assert_eq!(h.runs[0].date, 14);
        assert_eq!(h.records.iter().filter(|r| r.test == "sim").count(), 10);
        assert_eq!(h.records.iter().filter(|r| r.test == "disk").count(), 10);
        assert_eq!(h.records[0].test, "mem");
        let back: History = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back, h);
    }
}
