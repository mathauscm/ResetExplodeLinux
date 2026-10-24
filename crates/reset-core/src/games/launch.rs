//! Perfil de inicialização por jogo (portado do GameLauncher.BuildArgs e do GameProfile do Windows).
//! No Windows o app aplicava prioridade, plano de energia e gravava as opções no localconfig.vdf. No Linux o
//! equivalente cabe numa linha só de "Opções de inicialização" da Steam
//! (`VAR=1 gamemoderun mangohud %command% -argumentos`), que o app monta, mostra e copia; quem cola na Steam
//! é o usuário. Os perfis ficam em ~/.config/reset-explode/games.json.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LaunchProfile {
    /// "Prioridade alta": o GameMode (gamemoderun) aumenta a prioridade do jogo e põe a CPU em desempenho.
    pub gamemode: bool,
    /// Overlay de FPS e sensores (MangoHud).
    pub mangohud: bool,
    /// Proton + NVIDIA: libera DLSS e Reflex (PROTON_ENABLE_NVAPI=1).
    pub nvapi: bool,
    /// NVIDIA: não apaga o cache de shaders ao passar do limite (menos travadas depois de atualizar o driver).
    pub shader_cache: bool,
    /// default | full | borderless | window
    pub win_mode: String,
    /// default | 1280x720 | 1920x1080 | 2560x1440 | 3840x2160
    pub res: String,
    /// 0 = não definir
    pub hz: u32,
    /// source | unity | unreal
    pub style: String,
    pub extra: String,
}

impl Default for LaunchProfile {
    fn default() -> Self {
        Self {
            gamemode: false,
            mangohud: false,
            nvapi: false,
            shader_cache: false,
            win_mode: "default".into(),
            res: "default".into(),
            hz: 0,
            style: "source".into(),
            extra: String::new(),
        }
    }
}

/// Atalhos de argumentos extras (mesmos do Windows).
#[derive(Debug, Clone, Serialize)]
pub struct ExtraPreset {
    pub label: &'static str,
    pub token: &'static str,
    pub hint: &'static str,
}

pub const EXTRA_PRESETS: &[ExtraPreset] = &[
    ExtraPreset { label: "Pular intro (-novid)", token: "-novid", hint: "" },
    ExtraPreset { label: "Prioridade alta (-high)", token: "-high", hint: "Jogos Source: pede prioridade alta ao sistema. No Linux, o GameMode faz isso melhor." },
    ExtraPreset { label: "Sem limite de FPS (+fps_max 0)", token: "+fps_max 0", hint: "" },
    ExtraPreset { label: "Sem joystick (-nojoy)", token: "-nojoy", hint: "" },
    ExtraPreset { label: "Abrir console (-console)", token: "-console", hint: "" },
    ExtraPreset {
        label: "Programas de terceiros (-allow_third_party_software)",
        token: "-allow_third_party_software",
        hint: "CS2: libera programas que o modo confiável bloqueia, como a captura de jogo do OBS, ReShade e alguns overlays. \
               Atenção: a Valve diz que usar esta opção pode reduzir o seu Fator de Confiança (Trust Factor) no matchmaking.",
    },
];

/// A linha montada, em partes: a Steam usa tudo junto; Heroic e Lutris têm campos separados.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LaunchLine {
    /// Texto para colar em Propriedades → Geral → Opções de inicialização ("" = nenhuma).
    pub steam: String,
    pub env: Vec<String>,
    pub wrappers: Vec<String>,
    pub args: String,
}

/// Argumentos do jogo (modo de janela, resolução, taxa e extras) no estilo da engine escolhida.
pub fn build_args(p: &LaunchProfile) -> String {
    let mut a: Vec<String> = Vec::new();
    let (w, h) = p
        .res
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
        .unwrap_or((0, 0));
    match p.style.as_str() {
        "unity" => {
            match p.win_mode.as_str() {
                "full" => a.push("-screen-fullscreen 1".into()),
                "window" => a.push("-screen-fullscreen 0".into()),
                "borderless" => a.push("-popupwindow".into()),
                _ => {}
            }
            if w > 0 {
                a.push(format!("-screen-width {w} -screen-height {h}"));
            }
        }
        "unreal" => {
            match p.win_mode.as_str() {
                "full" => a.push("-fullscreen".into()),
                "window" | "borderless" => a.push("-windowed".into()),
                _ => {}
            }
            if w > 0 {
                a.push(format!("-ResX={w} -ResY={h}"));
            }
        }
        _ => {
            // source / CS2
            match p.win_mode.as_str() {
                "full" => a.push("-fullscreen".into()),
                "window" => a.push("-windowed".into()),
                "borderless" => a.push("-windowed -noborder".into()),
                _ => {}
            }
            if w > 0 {
                a.push(format!("-w {w} -h {h}"));
            }
            if p.hz > 0 {
                a.push(format!("-freq {}", p.hz));
            }
        }
    }
    let extra = p.extra.split_whitespace().collect::<Vec<_>>().join(" ");
    if !extra.is_empty() {
        a.push(extra);
    }
    a.join(" ")
}

/// Monta a linha completa. `proton` liga as variáveis que só fazem sentido em jogos de Windows.
pub fn build(p: &LaunchProfile, proton: bool) -> LaunchLine {
    let mut env = Vec::new();
    if p.nvapi && proton {
        env.push("PROTON_ENABLE_NVAPI=1".to_string());
    }
    if p.shader_cache {
        env.push("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1".to_string());
    }
    let mut wrappers = Vec::new();
    if p.gamemode {
        wrappers.push("gamemoderun".to_string());
    }
    if p.mangohud {
        wrappers.push("mangohud".to_string());
    }
    let args = build_args(p);
    let steam = if env.is_empty() && wrappers.is_empty() && args.is_empty() {
        String::new()
    } else {
        let mut parts: Vec<&str> = env.iter().chain(&wrappers).map(String::as_str).collect();
        parts.push("%command%");
        if !args.is_empty() {
            parts.push(&args);
        }
        parts.join(" ")
    };
    LaunchLine { steam, env, wrappers, args }
}

// ---------------------------------------------------------------- perfis salvos

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    profiles: HashMap<String, LaunchProfile>,
}

fn path() -> Option<PathBuf> {
    Some(super::config_dir()?.join("games.json"))
}

fn load_store() -> Store {
    path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn profile(key: &str) -> LaunchProfile {
    load_store().profiles.remove(key).unwrap_or_default()
}

/// Grava o perfil do jogo. O perfil padrão (nada escolhido) apaga a entrada.
pub fn save_profile(key: &str, p: &LaunchProfile) -> Result<(), String> {
    let path = path().ok_or("pasta de configuração não encontrada")?;
    let mut s = load_store();
    if *p == LaunchProfile::default() {
        s.profiles.remove(key);
    } else {
        s.profiles.insert(key.to_string(), p.clone());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("não deu para salvar o perfil: {e}"))
}

/// Jogos com perfil salvo (para marcar na grade).
pub fn keys_with_profile() -> Vec<String> {
    load_store().profiles.into_keys().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> LaunchProfile {
        LaunchProfile::default()
    }

    #[test]
    fn empty_profile_means_no_options() {
        let l = build(&p(), true);
        assert_eq!(l.steam, "");
        assert!(l.env.is_empty() && l.wrappers.is_empty() && l.args.is_empty());
    }

    #[test]
    fn args_per_engine() {
        let mut x = LaunchProfile { win_mode: "borderless".into(), res: "1920x1080".into(), hz: 144, ..p() };
        assert_eq!(build_args(&x), "-windowed -noborder -w 1920 -h 1080 -freq 144");
        x.style = "unity".into();
        assert_eq!(build_args(&x), "-popupwindow -screen-width 1920 -screen-height 1080");
        x.style = "unreal".into();
        x.win_mode = "full".into();
        assert_eq!(build_args(&x), "-fullscreen -ResX=1920 -ResY=1080");
        let y = LaunchProfile { res: "abc".into(), extra: "  -novid   -allow_third_party_software ".into(), ..p() };
        assert_eq!(build_args(&y), "-novid -allow_third_party_software");
    }

    #[test]
    fn full_steam_line() {
        let x = LaunchProfile { gamemode: true, mangohud: true, nvapi: true, shader_cache: true, extra: "-novid".into(), ..p() };
        let l = build(&x, true);
        assert_eq!(l.steam, "PROTON_ENABLE_NVAPI=1 __GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 gamemoderun mangohud %command% -novid");
        // jogo nativo: a variável do Proton não entra
        let l = build(&x, false);
        assert_eq!(l.steam, "__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 gamemoderun mangohud %command% -novid");
        let only_gm = LaunchProfile { gamemode: true, ..p() };
        assert_eq!(build(&only_gm, false).steam, "gamemoderun %command%");
    }

    #[test]
    fn profile_json_tolerates_missing_fields() {
        let x: LaunchProfile = serde_json::from_str(r#"{"gamemode": true}"#).unwrap();
        assert!(x.gamemode);
        assert_eq!(x.style, "source");
    }
}
