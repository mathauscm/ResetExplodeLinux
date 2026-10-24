//! Detector de gargalo (portado do BottleneckAnalyzer.cs): equilíbrio teórico GPU × CPU, classificação de cada
//! leitura dos sensores e o relatório de uma sessão de jogo. No Windows o FPS vinha do ETW; no Linux não há um
//! equivalente sem injetar nada no jogo, então a análise usa só os sensores (o FPS fica com o Overlay/MangoHud).

use crate::sensors::Snapshot;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Limiter {
    Gpu,
    Cpu,
    Vram,
    Ram,
    Thermal,
    Capped,
}

/// Uma leitura (1/s) de tudo que pode limitar o FPS.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LoadSample {
    pub gpu_load: Option<f64>,
    pub cpu_total: Option<f64>,
    pub cpu_max_core: Option<f64>,
    /// "thread 5"
    pub busiest_core: Option<String>,
    pub vram_pct: Option<f64>,
    pub ram_pct: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub cpu_temp: Option<f64>,
    /// NVML diz que o clock caiu por temperatura.
    pub gpu_thermal_cap: bool,
    /// O que segura o clock da GPU agora ("Energia", "Térmico"...).
    pub gpu_perfcap: Vec<String>,
}

impl LoadSample {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        let m = &s.summary;
        // núcleo (thread) mais ocupado e qual é; as linhas "cpu/load/N" vêm do /proc/stat
        let busiest = s
            .rows
            .iter()
            .filter(|r| r.id.strip_prefix("cpu/load/").is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())))
            .max_by(|a, b| a.value.total_cmp(&b.value));
        let g = m.gpu.as_ref();
        let vram_pct = g.and_then(|g| match (g.vram_used, g.vram_total) {
            (Some(u), Some(t)) if t > 0 => Some(100.0 * u as f64 / t as f64),
            _ => None,
        });
        let perfcap = g.map(|g| g.perfcap.clone()).unwrap_or_default();
        LoadSample {
            gpu_load: g.and_then(|g| g.load_pct).map(f64::from),
            cpu_total: m.cpu_load_pct,
            cpu_max_core: busiest.map(|r| r.value).or(m.cpu_top_core_pct),
            busiest_core: busiest.map(|r| format!("thread {}", r.id.trim_start_matches("cpu/load/"))),
            vram_pct,
            ram_pct: (m.ram_total > 0).then(|| 100.0 * m.ram_used as f64 / m.ram_total as f64),
            gpu_temp: g.and_then(|g| g.temp_c).map(f64::from),
            cpu_temp: m.cpu_temp_c,
            gpu_thermal_cap: perfcap.iter().any(|p| p.starts_with("Térmico")),
            gpu_perfcap: perfcap,
        }
    }

    /// Regras (na ordem): VRAM cheia → térmico → GPU ≥ 90% (normal em jogo pesado) → CPU/núcleo ≥ 85% com GPU sobrando →
    /// RAM cheia → nada no limite (trava de FPS, V-Sync ou menu).
    pub fn classify(&self) -> Limiter {
        let ge = |v: Option<f64>, t: f64| v.is_some_and(|v| v >= t);
        if ge(self.vram_pct, 97.0) {
            Limiter::Vram
        } else if self.gpu_thermal_cap || ge(self.cpu_temp, 95.0) {
            Limiter::Thermal
        } else if ge(self.gpu_load, 90.0) {
            Limiter::Gpu
        } else if ge(self.cpu_max_core, 85.0) || ge(self.cpu_total, 85.0) {
            Limiter::Cpu
        } else if ge(self.ram_pct, 92.0) {
            Limiter::Ram
        } else {
            Limiter::Capped
        }
    }
}

pub fn title(l: Limiter) -> &'static str {
    match l {
        Limiter::Gpu => "GPU no limite",
        Limiter::Cpu => "Gargalo de CPU",
        Limiter::Vram => "Memória de vídeo cheia",
        Limiter::Ram => "Memória RAM cheia",
        Limiter::Thermal => "Limitado por temperatura",
        Limiter::Capped => "Nada no limite",
    }
}

pub fn advice(l: Limiter) -> &'static str {
    match l {
        Limiter::Gpu => "Normal em jogos pesados: a placa de vídeo está sendo bem aproveitada. Para mais FPS, reduza \
            resolução ou qualidade gráfica, ou ative DLSS/FSR.",
        Limiter::Cpu => "A CPU (ou um núcleo dela) não acompanha a GPU. Subir resolução/gráficos quase não custa FPS. \
            Para ganhar FPS: reduza opções que pesam na CPU (distância de visão, multidões, física), feche programas \
            em segundo plano, rode o jogo com o GameMode e confira se o XMP/EXPO da RAM está ativo na BIOS.",
        Limiter::Vram => "A memória da placa de vídeo encheu, o que causa travadas. Reduza a qualidade das texturas \
            ou a resolução.",
        Limiter::Ram => "Pouca RAM livre: o Linux passa a usar a swap/zram e o jogo engasga. Feche navegadores e outros programas.",
        Limiter::Thermal => "Temperatura alta está derrubando o clock. Verifique ventilação do gabinete, poeira e pasta térmica.",
        Limiter::Capped => "Nem CPU nem GPU estão no limite: provavelmente há trava de FPS, V-Sync/G-Sync limitando, ou o jogo \
            está em menu/pausa. Normal se o FPS já é o desejado.",
    }
}

/// Os limites que pedem atenção (a GPU no limite é o cenário ideal; "nada no limite" é neutro).
pub fn is_alert(l: Limiter) -> bool {
    matches!(l, Limiter::Cpu | Limiter::Vram | Limiter::Ram | Limiter::Thermal)
}

#[derive(Debug, Clone, Serialize)]
pub struct Balance {
    pub ratio: f64,
    /// Posição do marcador de 0 (limita a GPU) a 1 (limita a CPU), escala logarítmica de 0,4× a 2,5×.
    pub pos: f64,
    pub text: &'static str,
}

/// Equilíbrio teórico pelo desempenho relativo (GPU: RTX 4070 = 100; CPU: Ryzen 7 7700X = 100).
pub fn balance(gpu_idx: f64, cpu_idx: f64) -> Balance {
    let ratio = gpu_idx / cpu_idx.max(1.0);
    let text = match ratio {
        r if r > 1.8 => "GPU muito mais forte que a CPU: em boa parte dos jogos a CPU vai limitar o FPS, principalmente em 1080p.",
        r if r > 1.3 => "GPU um pouco à frente da CPU: jogos competitivos em 1080p tendem a ficar limitados pela CPU; \
            em 1440p/4K a GPU volta a ser o limite.",
        r if r >= 0.7 => "Hardware equilibrado: na maioria dos jogos a GPU será o limite, que é o cenário ideal.",
        r if r > 0.4 => "CPU mais forte que a GPU: a placa de vídeo é o limite. Upgrade de GPU traria o maior ganho.",
        _ => "GPU muito fraca para esta CPU: a placa de vídeo limita quase tudo.",
    };
    let pos = ((ratio.max(1e-3).ln() - 0.4f64.ln()) / (2.5f64.ln() - 0.4f64.ln())).clamp(0.0, 1.0);
    Balance { ratio, pos, text }
}

// ---------------------------------------------------------------- sessão

/// Acumula amostras durante uma sessão de jogo e produz o relatório.
#[derive(Default)]
pub struct Session {
    samples: Vec<(LoadSample, Limiter)>,
    /// Jogos vistos rodando durante a sessão (na ordem em que apareceram).
    pub games: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Share {
    pub limiter: Limiter,
    pub title: &'static str,
    pub pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub samples: usize,
    pub main: Limiter,
    pub title: String,
    pub advice: &'static str,
    /// "GPU média 97%", "VRAM máx. 88%"...
    pub summary: Vec<String>,
    pub distribution: Vec<Share>,
    pub recommendations: Vec<String>,
    pub games: Vec<String>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, s: LoadSample) {
        let l = s.classify();
        self.samples.push((s, l));
    }

    pub fn count(&self) -> usize {
        self.samples.len()
    }

    pub fn note_game(&mut self, name: &str) {
        if !self.games.iter().any(|g| g == name) {
            self.games.push(name.to_string());
        }
    }

    /// % do tempo em cada limite, do maior para o menor.
    pub fn distribution(&self) -> Vec<Share> {
        let n = self.samples.len().max(1) as f64;
        let mut out: Vec<Share> = Vec::new();
        for (_, l) in &self.samples {
            match out.iter_mut().find(|s| s.limiter == *l) {
                Some(s) => s.pct += 100.0 / n,
                None => out.push(Share { limiter: *l, title: title(*l), pct: 100.0 / n }),
            }
        }
        out.sort_by(|a, b| b.pct.total_cmp(&a.pct));
        out
    }

    fn values(&self, f: impl Fn(&LoadSample) -> Option<f64>) -> Vec<f64> {
        self.samples.iter().filter_map(|(s, _)| f(s)).collect()
    }

    pub fn avg(&self, f: impl Fn(&LoadSample) -> Option<f64>) -> Option<f64> {
        let v = self.values(f);
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    }

    pub fn max(&self, f: impl Fn(&LoadSample) -> Option<f64>) -> Option<f64> {
        self.values(f).into_iter().reduce(f64::max)
    }

    fn pct_of(&self, pred: impl Fn(&LoadSample) -> bool) -> f64 {
        let n = self.samples.len().max(1) as f64;
        100.0 * self.samples.iter().filter(|(s, _)| pred(s)).count() as f64 / n
    }

    pub fn report(&self) -> Report {
        let dist = self.distribution();
        let main = dist.first().map(|s| s.limiter).unwrap_or(Limiter::Capped);
        let mut summary = Vec::new();
        let mut put = |label: &str, v: Option<f64>, unit: &str| {
            if let Some(v) = v {
                summary.push(format!("{label} {v:.0}{unit}"));
            }
        };
        put("GPU média", self.avg(|s| s.gpu_load), "%");
        put("CPU média", self.avg(|s| s.cpu_total), "%");
        put("núcleo mais ocupado", self.avg(|s| s.cpu_max_core), "%");
        put("VRAM máx.", self.max(|s| s.vram_pct), "%");
        put("RAM máx.", self.max(|s| s.ram_pct), "%");
        put("GPU máx.", self.max(|s| s.gpu_temp), " °C");
        put("CPU máx.", self.max(|s| s.cpu_temp), " °C");

        Report {
            samples: self.samples.len(),
            main,
            title: format!("Resultado: {}", title(main)),
            advice: advice(main),
            summary,
            recommendations: self.recommendations(main, &dist),
            distribution: dist,
            games: self.games.clone(),
        }
    }

    /// Dicas a partir dos números da sessão, além do conselho do limite principal.
    fn recommendations(&self, main: Limiter, dist: &[Share]) -> Vec<String> {
        let mut r = Vec::new();
        let share = |l: Limiter| dist.iter().find(|s| s.limiter == l).map(|s| s.pct).unwrap_or(0.0);
        let gpu_avg = self.avg(|s| s.gpu_load).unwrap_or(0.0);

        if self.games.is_empty() && main == Limiter::Capped && gpu_avg < 30.0 {
            r.push("Nenhum jogo foi detectado e a GPU ficou quase parada: abra o jogo e rode a análise de novo durante a partida.".into());
        }
        if main == Limiter::Capped && gpu_avg >= 30.0 {
            r.push("O jogo rodou abaixo do que o PC aguenta: se quiser mais FPS, desligue o V-Sync ou aumente o limite de FPS do jogo.".into());
        }
        if share(Limiter::Cpu) >= 20.0 {
            let core = self.avg(|s| s.cpu_max_core).unwrap_or(0.0);
            let total = self.avg(|s| s.cpu_total).unwrap_or(0.0);
            if core >= 80.0 && total < 60.0 {
                r.push(format!(
                    "Uma thread ficou em {core:.0}% enquanto a CPU toda ficou em {total:.0}%: o jogo depende de um núcleo só. \
                     Clock e cache da CPU contam mais aqui do que número de núcleos."
                ));
            }
            r.push("Para aliviar a CPU: GameMode (gamemoderun) ativo, navegador fechado e o perfil de energia em Desempenho.".into());
        }
        if self.max(|s| s.vram_pct).is_some_and(|v| v >= 90.0) {
            r.push("A VRAM chegou perto do limite: em placas de 8 GB, texturas em Alto (em vez de Ultra) evitam travadas.".into());
        }
        if self.max(|s| s.gpu_temp).is_some_and(|t| t >= 83.0) {
            r.push("A GPU passou de 83 °C: uma curva de ventoinha mais agressiva ou mais fluxo de ar no gabinete ajudam a manter o clock.".into());
        }
        if self.max(|s| s.cpu_temp).is_some_and(|t| t >= 90.0) {
            r.push("A CPU passou de 90 °C: confira o cooler e a pasta térmica.".into());
        }
        if self.max(|s| s.ram_pct).is_some_and(|v| v >= 85.0) {
            r.push("A RAM passou de 85% de uso: feche programas em segundo plano antes de jogar.".into());
        }
        let power = self.pct_of(|s| s.gpu_perfcap.iter().any(|p| p == "Energia"));
        if main == Limiter::Gpu && power >= 50.0 {
            r.push(format!(
                "Em {power:.0}% do tempo a GPU estava no limite de energia (normal com a GPU a 100%). \
                 Undervolt pode dar o mesmo FPS com menos calor e barulho."
            ));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuLive;
    use crate::sensors::{Kind, SensorRow, Summary};

    fn sample(gpu: f64, cpu: f64, core: f64) -> LoadSample {
        LoadSample { gpu_load: Some(gpu), cpu_total: Some(cpu), cpu_max_core: Some(core), vram_pct: Some(50.0), ram_pct: Some(40.0), ..Default::default() }
    }

    #[test]
    fn classify_rules_in_order() {
        assert_eq!(sample(98.0, 30.0, 50.0).classify(), Limiter::Gpu);
        assert_eq!(sample(60.0, 40.0, 95.0).classify(), Limiter::Cpu);
        assert_eq!(sample(40.0, 20.0, 30.0).classify(), Limiter::Capped);
        assert_eq!(LoadSample { vram_pct: Some(98.0), ..sample(99.0, 10.0, 10.0) }.classify(), Limiter::Vram);
        assert_eq!(LoadSample { gpu_thermal_cap: true, ..sample(99.0, 10.0, 10.0) }.classify(), Limiter::Thermal);
        assert_eq!(LoadSample { cpu_temp: Some(96.0), ..sample(50.0, 10.0, 10.0) }.classify(), Limiter::Thermal);
        assert_eq!(LoadSample { ram_pct: Some(95.0), ..sample(50.0, 10.0, 10.0) }.classify(), Limiter::Ram);
        // sem GPU NVIDIA (sem leituras) não quebra
        assert_eq!(LoadSample::default().classify(), Limiter::Capped);
    }

    #[test]
    fn from_snapshot_finds_busiest_thread() {
        let row = |id: &str, v: f64| SensorRow { id: id.into(), hardware: "CPU".into(), name: id.into(), kind: Kind::Load, value: v, unit: "%", min: v, max: v };
        let snap = Snapshot {
            summary: Summary {
                cpu_load_pct: Some(30.0),
                cpu_temp_c: Some(70.0),
                ram_used: 8,
                ram_total: 32,
                gpu: Some(GpuLive { load_pct: Some(99.0), vram_used: Some(6), vram_total: Some(8), perfcap: vec!["Energia".into()], ..Default::default() }),
                ..Default::default()
            },
            rows: vec![row("cpu/load", 30.0), row("cpu/load/0", 20.0), row("cpu/load/7", 91.0), row("cpu/load/3", 40.0)],
        };
        let s = LoadSample::from_snapshot(&snap);
        assert_eq!(s.cpu_max_core, Some(91.0));
        assert_eq!(s.busiest_core.as_deref(), Some("thread 7"));
        assert_eq!(s.vram_pct, Some(75.0));
        assert_eq!(s.ram_pct, Some(25.0));
        assert!(!s.gpu_thermal_cap);
        assert_eq!(s.classify(), Limiter::Gpu);
    }

    #[test]
    fn balance_scale() {
        let b = balance(70.0, 77.0); // RTX 4060 + Ryzen 7 5700X
        assert!(b.text.starts_with("Hardware equilibrado"));
        assert!((b.pos - 0.45).abs() < 0.05);
        assert_eq!(balance(265.0, 73.0).pos, 1.0);
        assert!(balance(265.0, 73.0).text.starts_with("GPU muito mais forte"));
        assert_eq!(balance(6.0, 140.0).pos, 0.0);
        assert!((balance(100.0, 100.0).pos - 0.5).abs() < 1e-9);
    }

    #[test]
    fn session_report() {
        let mut s = Session::new();
        for _ in 0..6 {
            s.add(sample(99.0, 40.0, 60.0));
        }
        for _ in 0..3 {
            s.add(sample(60.0, 45.0, 95.0));
        }
        s.add(LoadSample { vram_pct: Some(98.0), ..sample(99.0, 40.0, 60.0) });
        s.note_game("Red Dead Redemption 2");
        s.note_game("Red Dead Redemption 2");
        let r = s.report();
        assert_eq!(r.samples, 10);
        assert_eq!(r.main, Limiter::Gpu);
        let pcts: Vec<(Limiter, i64)> = r.distribution.iter().map(|d| (d.limiter, d.pct.round() as i64)).collect();
        assert_eq!(pcts, [(Limiter::Gpu, 60), (Limiter::Cpu, 30), (Limiter::Vram, 10)]);
        assert!(r.summary.iter().any(|x| x == "VRAM máx. 98%"));
        assert!(r.recommendations.iter().any(|x| x.contains("VRAM")));
        assert!(r.recommendations.iter().any(|x| x.contains("GameMode")));
        assert_eq!(r.games, ["Red Dead Redemption 2"]);
    }

    #[test]
    fn idle_session_says_no_game() {
        let mut s = Session::new();
        s.add(sample(3.0, 5.0, 20.0));
        let r = s.report();
        assert_eq!(r.main, Limiter::Capped);
        assert!(r.recommendations[0].contains("Nenhum jogo"));
    }
}
