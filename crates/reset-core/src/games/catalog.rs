//! Catálogo de jogos e estimativa local de FPS (portado do GameCatalog.cs e do GameRow.cs).
//! Valores de referência APROXIMADOS (compilados de benchmarks públicos), usados quando a API da HowManyFPS
//! não está configurada: `gpu_ref` = FPS médio em 1080p/Alto numa RTX 4070 (índice 100); `cpu_cap` = teto de FPS
//! limitado pela CPU num Ryzen 7 7700X (índice 100); `engine_cap` = trava de FPS do próprio jogo (0 = sem trava).

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct GameInfo {
    pub name: &'static str,
    pub genre: &'static str,
    pub gpu_ref: f64,
    pub cpu_cap: f64,
    pub engine_cap: f64,
}

const fn g(name: &'static str, genre: &'static str, gpu_ref: f64, cpu_cap: f64) -> GameInfo {
    GameInfo { name, genre, gpu_ref, cpu_cap, engine_cap: 0.0 }
}

// O nome é enviado à API como está, então segue a grafia usada pela HowManyFPS.
pub const GAMES: [GameInfo; 32] = [
    g("Counter-Strike 2", "FPS competitivo", 340.0, 420.0),
    g("Valorant", "FPS competitivo", 600.0, 650.0),
    g("Fortnite", "Battle royale", 160.0, 250.0),
    g("League of Legends", "MOBA", 420.0, 480.0),
    g("Dota 2", "MOBA", 260.0, 280.0),
    g("Apex Legends", "Battle royale", 220.0, 300.0),
    g("PUBG: Battlegrounds", "Battle royale", 180.0, 220.0),
    g("Call of Duty: Black Ops 6", "FPS", 140.0, 220.0),
    g("Call of Duty: Warzone", "Battle royale", 130.0, 200.0),
    g("Battlefield 6", "FPS", 120.0, 170.0),
    g("Rainbow Six Siege", "FPS tático", 350.0, 430.0),
    g("Overwatch 2", "FPS heroico", 280.0, 380.0),
    g("Marvel Rivals", "FPS heroico", 110.0, 160.0),
    g("Rocket League", "Esporte", 400.0, 450.0),
    g("EA Sports FC 25", "Esporte", 220.0, 260.0),
    g("Minecraft", "Sandbox", 300.0, 340.0),
    g("GTA V", "Mundo aberto", 160.0, 190.0),
    g("Red Dead Redemption 2", "Mundo aberto", 95.0, 160.0),
    g("Cyberpunk 2077", "RPG mundo aberto", 95.0, 150.0),
    g("The Witcher 3: Wild Hunt", "RPG", 120.0, 160.0),
    GameInfo { name: "Elden Ring", genre: "RPG de ação", gpu_ref: 110.0, cpu_cap: 140.0, engine_cap: 60.0 },
    g("Baldur's Gate 3", "RPG", 110.0, 120.0),
    g("Hogwarts Legacy", "RPG de ação", 90.0, 120.0),
    g("Starfield", "RPG espacial", 65.0, 90.0),
    g("Black Myth: Wukong", "Ação", 70.0, 140.0),
    g("Alan Wake 2", "Terror / ação", 60.0, 140.0),
    g("God of War Ragnarök", "Ação", 110.0, 170.0),
    g("Forza Horizon 5", "Corrida", 140.0, 200.0),
    g("Helldivers 2", "Cooperativo", 90.0, 110.0),
    g("Monster Hunter Wilds", "Ação", 60.0, 85.0),
    g("Assassin's Creed Shadows", "Ação mundo aberto", 65.0, 110.0),
    g("Borderlands 4", "Looter shooter", 55.0, 110.0),
];

// Desempenho relativo em jogos (raster), RTX 4070 = 100. Aproximado.
const GPU_INDEX: &[(&str, f64)] = &[
    ("RTX 5090", 265.0), ("RTX 5080", 175.0), ("RTX 5070 Ti", 150.0), ("RTX 5070", 118.0), ("RTX 5060 Ti", 95.0), ("RTX 5060", 85.0),
    ("RTX 4090", 210.0), ("RTX 4080 SUPER", 160.0), ("RTX 4080", 157.0), ("RTX 4070 Ti SUPER", 136.0), ("RTX 4070 Ti", 124.0),
    ("RTX 4070 SUPER", 116.0), ("RTX 4070", 100.0), ("RTX 4060 Ti", 82.0), ("RTX 4060", 70.0),
    ("RTX 3090 Ti", 135.0), ("RTX 3090", 125.0), ("RTX 3080 Ti", 122.0), ("RTX 3080", 112.0), ("RTX 3070 Ti", 92.0), ("RTX 3070", 86.0),
    ("RTX 3060 Ti", 78.0), ("RTX 3060", 60.0), ("RTX 3050", 40.0), ("GTX 1660", 38.0), ("GTX 1650", 28.0),
    ("RX 9070 XT", 160.0), ("RX 9070", 140.0), ("RX 7900 XTX", 170.0), ("RX 7900 XT", 150.0), ("RX 7900 GRE", 128.0),
    ("RX 7800 XT", 115.0), ("RX 7700 XT", 98.0), ("RX 7600 XT", 72.0), ("RX 7600", 70.0),
    ("RX 6950 XT", 125.0), ("RX 6800 XT", 112.0), ("RX 6700 XT", 82.0), ("RX 6600", 58.0),
    ("Arc B580", 80.0), ("Arc A770", 68.0),
    ("Radeon(TM) Graphics", 6.0), ("Radeon Graphics", 6.0),
];

// Desempenho relativo em jogos (limitado pela CPU), Ryzen 7 7700X = 100. Aproximado.
const CPU_INDEX: &[(&str, f64)] = &[
    ("9950X3D", 138.0), ("9800X3D", 140.0), ("7950X3D", 126.0), ("7800X3D", 128.0), ("5800X3D", 95.0), ("5700X3D", 90.0),
    ("9950X", 112.0), ("9900X", 110.0), ("9700X", 108.0), ("9600X", 104.0), ("7950X", 104.0), ("7900X", 102.0), ("7700X", 100.0),
    ("7700", 97.0), ("7600X", 96.0), ("7600", 93.0), ("5950X", 82.0), ("5900X", 81.0), ("5800X", 80.0), ("5700X", 77.0), ("5600X", 76.0), ("5600", 73.0),
    ("285K", 112.0), ("265K", 108.0), ("245K", 100.0), ("14900K", 116.0), ("14700K", 112.0), ("14600K", 104.0), ("13900K", 113.0),
    ("13700K", 108.0), ("13600K", 101.0), ("13400", 84.0), ("12900K", 98.0), ("12700K", 95.0), ("12600K", 90.0), ("12400", 78.0),
];

/// Índice de desempenho da GPU (RTX 4070 = 100), pelo nome completo ou curto.
pub fn gpu_perf(name: &str) -> Option<f64> {
    lookup(GPU_INDEX, name).or_else(|| lookup(GPU_INDEX, &short_gpu_name(name)))
}

/// Índice de desempenho da CPU em jogos (Ryzen 7 7700X = 100).
pub fn cpu_perf(name: &str) -> Option<f64> {
    lookup(CPU_INDEX, name)
}

/// A chave mais longa contida no nome vence ("RTX 5070 Ti" antes de "RTX 5070").
fn lookup(table: &[(&str, f64)], name: &str) -> Option<f64> {
    let low = name.to_lowercase();
    table.iter().filter(|(k, _)| low.contains(&k.to_lowercase())).max_by_key(|(k, _)| k.len()).map(|(_, v)| *v)
}

pub fn resolution_factor(res: &str) -> f64 {
    match res {
        "2560x1440" => 0.72,
        "3840x2160" => 0.42,
        _ => 1.0,
    }
}

pub fn preset_factor(preset: &str) -> f64 {
    match preset {
        "Baixo" => 1.6,
        "Médio" => 1.3,
        "Ultra" => 0.82,
        _ => 1.0,
    }
}

/// Upscaling (DLSS/FSR) só alivia a GPU; o ganho cresce com a resolução.
pub fn upscaling_factor(upscaling: &str, res: &str) -> f64 {
    let fhd = res == "1920x1080";
    match upscaling {
        "Quality" => if fhd { 1.25 } else { 1.4 },
        "Performance" => if fhd { 1.45 } else { 1.75 },
        _ => 1.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Limit {
    Gpu,
    Cpu,
    Engine,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Estimate {
    pub avg: f64,
    pub low1: f64,
    pub limiter: Limit,
}

/// FPS = menor entre o teto da GPU, o teto da CPU e a trava do jogo.
pub fn estimate(g: &GameInfo, gpu_idx: f64, cpu_idx: f64, res: &str, preset: &str, upscaling: &str) -> Estimate {
    let gpu_fps = g.gpu_ref * gpu_idx / 100.0 * resolution_factor(res) * preset_factor(preset) * upscaling_factor(upscaling, res);
    let cpu_fps = g.cpu_cap * cpu_idx / 100.0
        * match preset {
            "Baixo" => 1.1,
            "Ultra" => 0.95,
            _ => 1.0,
        };
    let mut fps = gpu_fps.min(cpu_fps);
    let mut limiter = if gpu_fps <= cpu_fps { Limit::Gpu } else { Limit::Cpu };
    if g.engine_cap > 0.0 && g.engine_cap < fps {
        fps = g.engine_cap;
        limiter = Limit::Engine;
    }
    Estimate { avg: fps.round(), low1: (fps * 0.75).round(), limiter }
}

/// Faixas comuns em calculadoras de FPS: 144+ excelente, 90+ ótimo, 60+ jogável.
pub fn rating(fps: f64) -> &'static str {
    match fps {
        f if f <= 0.0 => "",
        f if f >= 144.0 => "EXCELENTE",
        f if f >= 90.0 => "ÓTIMO",
        f if f >= 60.0 => "JOGÁVEL",
        f if f >= 30.0 => "FRACO",
        _ => "INJOGÁVEL",
    }
}

/// Nomes curtos como a API espera: "RTX 5070", "RX 9070 XT".
pub fn short_gpu_name(name: &str) -> String {
    let mut s = name.replace("(TM)", "").replace("(R)", "");
    for w in ["NVIDIA", "GeForce", "AMD", "Intel"] {
        s = s.replace(w, "");
    }
    if s.contains("Radeon RX") {
        s = s.replace("Radeon", ""); // "Radeon RX 9070 XT" → "RX 9070 XT"
    }
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.starts_with("Arc") && s.ends_with(" Graphics") {
        s.truncate(s.len() - " Graphics".len()); // "Arc B580 Graphics" → "Arc B580"
    }
    s // iGPU AMD fica "Radeon Graphics"
}

/// "AMD Ryzen 7 5700X 8-Core Processor" → "Ryzen 7 5700X"; "13th Gen Intel(R) Core(TM) i7-13700K" → "Core i7-13700K".
pub fn short_cpu_name(name: &str) -> String {
    let mut s = name.replace("AMD", "").replace("Intel(R)", "").replace("Core(TM)", "Core").replace("(R)", "").replace("(TM)", "");
    if let Some(i) = s.to_lowercase().find("-core") {
        if let Some(sp) = s[..i].rfind(' ') {
            s.truncate(sp);
        }
    }
    s = s.replace("Processor", "").replace("CPU", "");
    if let Some(at) = s.find('@') {
        s.truncate(at); // "Core i5-12400F CPU @ 2.50GHz"
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    // "13th Gen Core i7-13700K": tira a geração
    let skip = if words.len() > 2 && words[1] == "Gen" && words[0].ends_with("th") { 2 } else { 0 };
    words[skip..].join(" ")
}

/// Para comparar nomes da loja com o catálogo: só letras e números, minúsculas ("EA SPORTS FC™ 26" → "easportsfc26").
pub fn norm_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_32_unique_games() {
        let mut names: Vec<_> = GAMES.iter().map(|g| norm_name(g.name)).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 32);
    }

    #[test]
    fn indexes_prefer_longest_key() {
        assert_eq!(gpu_perf("NVIDIA GeForce RTX 4060"), Some(70.0));
        assert_eq!(gpu_perf("NVIDIA GeForce RTX 4060 Ti"), Some(82.0));
        assert_eq!(gpu_perf("NVIDIA GeForce RTX 4070 Ti SUPER"), Some(136.0));
        assert_eq!(gpu_perf("AMD Radeon RX 9070 XT"), Some(160.0));
        assert_eq!(cpu_perf("AMD Ryzen 7 5700X 8-Core Processor"), Some(77.0));
        assert_eq!(cpu_perf("AMD Ryzen 7 5700X3D 8-Core Processor"), Some(90.0));
        assert_eq!(cpu_perf("Processador desconhecido"), None);
    }

    #[test]
    fn short_names() {
        assert_eq!(short_gpu_name("NVIDIA GeForce RTX 4060"), "RTX 4060");
        assert_eq!(short_gpu_name("AMD Radeon RX 9070 XT"), "RX 9070 XT");
        assert_eq!(short_gpu_name("Intel(R) Arc(TM) B580 Graphics"), "Arc B580");
        assert_eq!(short_cpu_name("AMD Ryzen 7 5700X 8-Core Processor"), "Ryzen 7 5700X");
        assert_eq!(short_cpu_name("12th Gen Intel(R) Core(TM) i5-12400F CPU @ 2.50GHz"), "Core i5-12400F");
        assert_eq!(short_cpu_name("13th Gen Intel(R) Core(TM) i7-13700K"), "Core i7-13700K");
    }

    #[test]
    fn estimate_matches_windows_formula() {
        // RTX 4060 (70) + Ryzen 7 5700X (77), CS2 em 1080p/Alto nativo: GPU 340*0,70 = 238; CPU 420*0,77 = 323,4
        let cs2 = GAMES.iter().find(|g| g.name == "Counter-Strike 2").unwrap();
        let e = estimate(cs2, 70.0, 77.0, "1920x1080", "Alto", "Native");
        assert_eq!((e.avg, e.low1, e.limiter), (238.0, 179.0, Limit::Gpu));
        // em Baixo + DLSS Desempenho a GPU sobra e a CPU vira o limite
        let e = estimate(cs2, 70.0, 77.0, "1920x1080", "Baixo", "Performance");
        assert_eq!(e.limiter, Limit::Cpu);
        assert_eq!(e.avg, (420.0f64 * 0.77 * 1.1).round());
        // Elden Ring trava em 60
        let er = GAMES.iter().find(|g| g.name == "Elden Ring").unwrap();
        let e = estimate(er, 210.0, 140.0, "1920x1080", "Alto", "Native");
        assert_eq!((e.avg, e.limiter), (60.0, Limit::Engine));
        // 4K pesa mais que 1440p
        let rdr = GAMES.iter().find(|g| g.name == "Red Dead Redemption 2").unwrap();
        assert!(estimate(rdr, 70.0, 77.0, "3840x2160", "Alto", "Native").avg < estimate(rdr, 70.0, 77.0, "2560x1440", "Alto", "Native").avg);
    }

    #[test]
    fn ratings() {
        assert_eq!(rating(0.0), "");
        assert_eq!(rating(200.0), "EXCELENTE");
        assert_eq!(rating(90.0), "ÓTIMO");
        assert_eq!(rating(61.0), "JOGÁVEL");
        assert_eq!(rating(45.0), "FRACO");
        assert_eq!(rating(12.0), "INJOGÁVEL");
    }
}
