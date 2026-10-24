//! RESET BENCH: os mesmos testes do app do Windows.
//!
//! - `render`: path tracer de CPU (nota idêntica à do `BenchRenderer.cs`)
//! - `gamesim`: simulação de jogo na CPU (`GameSimBench.cs`)
//! - `memory`: latência/banda da RAM e carregamento de mapa (`MemoryBench.cs`)
//! - `disk`: sequencial e 4 KB aleatório sem cache (`DiskBench.cs`)
//! - `gpu`: cena de jogo, ray tracing e stress de GPU pelo programa `reset-gpu` (Vulkan, janela própria)
//! - `history`: últimos resultados em ~/.config/reset-explode/bench.json
//!
//! Tudo aqui é bloqueante e pesado: quem chama roda numa thread própria e cancela com um `AtomicBool`.

pub mod disk;
pub mod gamesim;
pub mod gpu;
pub mod history;
pub mod memory;
pub mod render;

/// Bytes em base64 (pixels que vão para a interface por evento).
pub fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Número com `d` casas no padrão brasileiro: 12345.6 → "12.345,6".
pub fn fmt_num(v: f64, d: usize) -> String {
    let s = format!("{:.*}", d, v.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push(',');
        out.push_str(frac);
    }
    if v < 0.0 && out.chars().any(|c| c.is_ascii_digit() && c != '0') {
        out.insert(0, '-');
    }
    out
}

/// Nome curto da CPU para o histórico: "AMD Ryzen 7 5700X 8-Core Processor" → "Ryzen 7 5700X".
pub fn short_cpu_name(model: &str) -> String {
    let clean = model.replace("(R)", "").replace("(TM)", "");
    let drop = ["AMD", "Intel", "CPU", "Processor"];
    let s = clean
        .split_whitespace()
        .take_while(|w| *w != "@" && *w != "with")
        .filter(|w| !drop.contains(w) && !w.to_lowercase().ends_with("-core"))
        .collect::<Vec<_>>()
        .join(" ");
    if s.is_empty() { model.trim().to_string() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeros_em_portugues() {
        assert_eq!(fmt_num(12345.64, 1), "12.345,6");
        assert_eq!(fmt_num(4500.0, 0), "4.500");
        assert_eq!(fmt_num(999.0, 0), "999");
        assert_eq!(fmt_num(0.456, 2), "0,46");
        assert_eq!(fmt_num(-1234.0, 0), "-1.234");
    }

    #[test]
    fn nome_curto() {
        assert_eq!(short_cpu_name("AMD Ryzen 7 5700X 8-Core Processor"), "Ryzen 7 5700X");
        assert_eq!(short_cpu_name("13th Gen Intel(R) Core(TM) i5-13600K"), "13th Gen Core i5-13600K");
        assert_eq!(short_cpu_name("Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz"), "Core i7-8700");
    }
}
