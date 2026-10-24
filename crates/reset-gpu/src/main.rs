//! reset-gpu: benchmarks e stress de GPU do RESET EXPLODE para Linux, numa janela própria em Vulkan.
//! Port do GpuGameBench.cs (cena de jogo), GpuRtBench.cs (ray tracing) e GpuBurn.cs (stress) do app do Windows.
//! Contrato com o app (linha de comando e uma linha JSON por evento no stdout): linux/README.md, seção "reset-gpu".

/// SPIR-V compilado pelo build.rs (a partir de `shaders/`), embutido no binário.
macro_rules! spv {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".spv"))
    };
}

mod burn;
mod ctx;
mod frames;
mod out;
mod rt;
mod scene;
mod window;

use anyhow::{bail, Context, Result};

const USO: &str = "uso: reset-gpu info | scene | rt | burn [--width N] [--height N] [--seconds S] [--fullscreen] \
                   [--preset normal|alta|extrema] [--msaa 0|4|8] [--intensity normal|alta|extrema] [--temp-limit C]";

/// Opções da linha de comando (as que não se aplicam ao modo são ignoradas).
pub struct Opts {
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub fullscreen: bool,
    /// rt: normal | alta | extrema
    pub preset: String,
    /// burn: 1 (sem MSAA), 4 ou 8
    pub msaa: u32,
    /// burn: normal | alta | extrema
    pub intensity: String,
    /// burn: para ao chegar nesta temperatura (°C)
    pub temp_limit: f32,
}

/// Lê as opções; valor fora do contrato vira erro com a explicação (e o app mostra a mensagem).
fn parse(mode: &str, args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        width: 1920,
        height: 1080,
        seconds: if mode == "burn" { 600.0 } else { 30.0 },
        fullscreen: false,
        preset: "alta".into(),
        msaa: 4,
        intensity: "alta".into(),
        temp_limit: 85.0,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--fullscreen" {
            o.fullscreen = true;
            continue;
        }
        let v = it.next().with_context(|| format!("{a} precisa de um valor. {USO}"))?;
        let num = |what: &str| -> Result<f64> {
            v.replace(',', ".").parse::<f64>().ok().filter(|x| x.is_finite()).with_context(|| format!("{what} inválido: {v}"))
        };
        match a.as_str() {
            "--width" => o.width = num("largura")? as u32,
            "--height" => o.height = num("altura")? as u32,
            "--seconds" => o.seconds = num("duração")?,
            "--temp-limit" => o.temp_limit = num("limite de temperatura")? as f32,
            "--msaa" => {
                o.msaa = match v.as_str() {
                    "0" | "1" => 1,
                    "2" => 2,
                    "4" => 4,
                    "8" => 8,
                    _ => bail!("MSAA inválido: {v} (use 0, 4 ou 8)"),
                }
            }
            "--preset" | "--intensity" => {
                let p = v.to_lowercase();
                if !["normal", "alta", "extrema"].contains(&p.as_str()) {
                    bail!("{a} inválido: {v} (use normal, alta ou extrema)");
                }
                if a == "--preset" {
                    o.preset = p
                } else {
                    o.intensity = p
                }
            }
            _ => bail!("opção desconhecida: {a}. {USO}"),
        }
    }
    if !(64..=7680).contains(&o.width) || !(64..=4320).contains(&o.height) {
        bail!("resolução fora do intervalo: {}x{} (de 64x64 a 7680x4320)", o.width, o.height);
    }
    if o.seconds <= 0.0 {
        bail!("a duração precisa ser maior que zero");
    }
    Ok(o)
}

fn run(args: &[String]) -> Result<()> {
    let Some(mode) = args.first() else { bail!(USO) };
    let o = parse(mode, &args[1..])?;
    match mode.as_str() {
        "info" => {
            out::info(&ctx::probe()?);
            Ok(())
        }
        "scene" => scene::run(&o),
        "rt" => rt::run(&o),
        "burn" => burn::run(&o),
        "-h" | "--help" | "help" => bail!(USO),
        _ => bail!("modo desconhecido: {mode}. {USO}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        out::error(&format!("{e:#}"));
        std::process::exit(1);
    }
}
