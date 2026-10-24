//! Overlay sobre os jogos. No Windows o app desenha a própria janela e lê o FPS de fora pelo ETW; no Linux não
//! dá para ler o FPS de outro processo assim, então o padrão é o MangoHud (camada Vulkan/OpenGL que roda dentro
//! do jogo). Aqui ficam o catálogo do que pode aparecer (o mesmo do `OverlayMetrics.cs`), a tradução para as
//! chaves do `MangoHud.conf` e os arquivos do usuário:
//!
//! - `~/.config/MangoHud/MangoHud.conf`: gerado a partir das escolhas; o arquivo que já existia ganha uma cópia
//!   antes de ser trocado (`MangoHud.conf.antes-do-reset-explode`).
//! - `~/.config/environment.d/90-reset-explode-mangohud.conf`: `MANGOHUD=1` para ligar em todos os jogos Vulkan
//!   (vale a partir do próximo login; apagar o arquivo desfaz).
//!
//! Todas as funções de arquivo recebem os caminhos (`Paths`), para os testes rodarem numa pasta temporária.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Item que pode aparecer no overlay. `mango` são as chaves do MangoHud que ele liga; `note` explica quando o
/// MangoHud mostra diferente do app do Windows.
#[derive(Debug, Clone, Serialize)]
pub struct Metric {
    pub key: &'static str,
    /// "FPS", "GPU", "VRAM", "CPU", "RAM" ou "Outros"
    pub group: &'static str,
    pub label: &'static str,
    pub default: bool,
    pub mango: &'static [&'static str],
    pub note: Option<&'static str>,
}

const fn m(key: &'static str, group: &'static str, label: &'static str, default: bool, mango: &'static [&'static str], note: Option<&'static str>) -> Metric {
    Metric { key, group, label, default, mango, note }
}

/// Catálogo, na ordem da tela (mesmas chaves do app do Windows).
pub const METRICS: &[Metric] = &[
    m("fps", "FPS", "FPS", true, &["fps"], None),
    m("frametime", "FPS", "Frametime (ms)", true, &["fps", "frametime"], Some("aparece ao lado do FPS")),
    m("low1", "FPS", "1% low", true, &["fps_metrics"], Some("precisa do MangoHud 0.7.2 ou mais novo")),
    m("graph", "FPS", "Gráfico de frametime", true, &["frame_timing"], None),
    m("gpu_load", "GPU", "Uso", true, &["gpu_stats"], None),
    m("gpu_temp", "GPU", "Temperatura", true, &["gpu_stats", "gpu_temp"], None),
    m("gpu_hotspot", "GPU", "Hotspot", false, &["gpu_stats", "gpu_junction_temp"], Some("placas NVIDIA recentes não informam o hotspot no Linux")),
    m("gpu_clock", "GPU", "Clock do núcleo", true, &["gpu_stats", "gpu_core_clock"], None),
    m("gpu_memclock", "GPU", "Clock da memória", false, &["vram", "gpu_mem_clock"], Some("aparece na linha da VRAM")),
    m("gpu_power", "GPU", "Consumo", true, &["gpu_stats", "gpu_power"], None),
    m("gpu_volt", "GPU", "Tensão do núcleo", false, &["gpu_stats", "gpu_voltage"], Some("só placas AMD informam a tensão no Linux")),
    m("gpu_fan", "GPU", "Ventoinha", false, &["gpu_stats", "gpu_fan"], None),
    m("gpu_perfcap", "GPU", "Limitador (NVIDIA)", false, &["throttling_status"], Some("só aparece quando a placa está limitando")),
    m("vram_used", "VRAM", "Usada (GB)", true, &["vram"], None),
    m("vram_pct", "VRAM", "Uso (%)", false, &["vram"], Some("o MangoHud mostra a VRAM em GB, não em %")),
    m("vram_temp", "VRAM", "Temperatura", false, &["vram", "gpu_mem_temp"], Some("placas NVIDIA GeForce não informam no Linux")),
    m("cpu_load", "CPU", "Uso total", true, &["cpu_stats"], None),
    m("cpu_maxcore", "CPU", "Núcleo mais ocupado", false, &["core_load", "core_bars"], Some("o MangoHud mostra a carga de cada núcleo, em barras")),
    m("cpu_temp", "CPU", "Temperatura", true, &["cpu_stats", "cpu_temp"], None),
    m("cpu_clock", "CPU", "Clock médio", true, &["cpu_stats", "cpu_mhz"], Some("o MangoHud mostra o clock do núcleo mais rápido")),
    m("cpu_clockmax", "CPU", "Clock máximo (núcleo)", false, &["cpu_stats", "cpu_mhz"], None),
    m("cpu_power", "CPU", "Consumo", true, &["cpu_stats", "cpu_power"], Some("em Ryzen precisa do módulo zenpower ou de leitura RAPL liberada")),
    m("ram_used", "RAM", "Usada (GB)", true, &["ram"], None),
    m("ram_pct", "RAM", "Uso (%)", false, &["ram"], Some("o MangoHud mostra a RAM em GB, não em %")),
    m("game", "Outros", "Nome do jogo", true, &["exec_name"], Some("nome do executável")),
    m("clock", "Outros", "Hora", false, &["time"], None),
];

/// Ordem em que as chaves vão para o arquivo. Com `legacy_layout=0` o MangoHud desenha os blocos na ordem do
/// arquivo; as chaves que não são blocos (gpu_temp, cpu_mhz...) só ligam valores dentro da linha do bloco.
const ORDER: &[&str] = &[
    "fps", "frametime", "fps_metrics", "frame_timing",
    "gpu_stats", "gpu_temp", "gpu_junction_temp", "gpu_core_clock", "gpu_power", "gpu_voltage", "gpu_fan", "throttling_status",
    "vram", "gpu_mem_clock", "gpu_mem_temp",
    "cpu_stats", "cpu_temp", "cpu_mhz", "cpu_power", "core_load", "core_bars",
    "ram", "exec_name", "time",
];

/// Valores que entram na linha "GPU" do MangoHud (o uso sempre aparece junto).
const GPU_ROW: &[&str] = &["gpu_stats", "gpu_temp", "gpu_junction_temp", "gpu_core_clock", "gpu_power", "gpu_voltage", "gpu_fan"];
const CPU_ROW: &[&str] = &["cpu_stats", "cpu_temp", "cpu_mhz", "cpu_power"];

pub fn defaults() -> Vec<String> {
    METRICS.iter().filter(|m| m.default).map(|m| m.key.to_string()).collect()
}

/// Presets da tela, iguais aos do Windows: "min" (SÓ FPS), "default" (PADRÃO), "all" (TUDO).
pub fn preset(name: &str) -> Vec<String> {
    match name {
        "min" => ["fps", "frametime", "low1"].map(String::from).to_vec(),
        "all" => METRICS.iter().map(|m| m.key.to_string()).collect(),
        _ => defaults(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    TL,
    TR,
    BL,
    BR,
}

impl Corner {
    fn mango(self) -> &'static str {
        match self {
            Corner::TL => "top-left",
            Corner::TR => "top-right",
            Corner::BL => "bottom-left",
            Corner::BR => "bottom-right",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "TL" => Corner::TL,
            "TR" => Corner::TR,
            "BL" => Corner::BL,
            "BR" => Corner::BR,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    /// Mesma proporção do app do Windows (0,85 / 1 / 1,3) sobre a fonte padrão do MangoHud (24).
    fn font_size(self) -> u32 {
        match self {
            Size::Small => 20,
            Size::Medium => 24,
            Size::Large => 31,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "small" => Size::Small,
            "medium" => Size::Medium,
            "large" => Size::Large,
            _ => return None,
        })
    }
}

/// Atalhos de mostrar/esconder oferecidos na tela (nomes de tecla do X11, como o MangoHud espera).
pub const TOGGLE_KEYS: &[(&str, &str)] = &[
    ("Shift_R+F12", "Shift direito + F12 (padrão do MangoHud)"),
    ("Shift_L+F12", "Shift esquerdo + F12"),
    ("Control_L+Shift_L+O", "Ctrl + Shift + O (igual ao app do Windows)"),
    ("Shift_L+F1", "Shift esquerdo + F1"),
    ("F12", "F12"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlaySettings {
    /// Chaves de `METRICS`.
    pub items: Vec<String>,
    pub corner: Corner,
    pub size: Size,
    /// Uma das chaves de `TOGGLE_KEYS`.
    pub toggle: String,
    /// Começa escondido; aparece com o atalho.
    #[serde(default)]
    pub start_hidden: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self { items: defaults(), corner: Corner::TL, size: Size::Medium, toggle: TOGGLE_KEYS[0].0.into(), start_hidden: false }
    }
}

impl OverlaySettings {
    /// Tira itens desconhecidos e repetidos, põe na ordem do catálogo e troca atalho inválido pelo padrão.
    pub fn sanitized(&self) -> Self {
        let items = METRICS.iter().filter(|m| self.items.iter().any(|i| i == m.key)).map(|m| m.key.to_string()).collect();
        let toggle = if TOGGLE_KEYS.iter().any(|(k, _)| *k == self.toggle) { self.toggle.clone() } else { TOGGLE_KEYS[0].0.into() };
        Self { items, toggle, ..self.clone() }
    }
}

const MARKER: &str = "# reset-explode:";

/// Chaves do MangoHud ligadas pelos itens escolhidos, sem repetir, na ordem em que vão para o arquivo.
pub fn mango_keys(items: &[String]) -> Vec<&'static str> {
    let wanted: Vec<&str> = METRICS.iter().filter(|m| items.iter().any(|i| i == m.key)).flat_map(|m| m.mango.iter().copied()).collect();
    ORDER.iter().copied().filter(|k| wanted.contains(k)).collect()
}

/// Texto completo do `MangoHud.conf` para as escolhas. As escolhas também vão numa linha de comentário
/// (`# reset-explode: ...`) para a tela reabrir como estava.
pub fn generate(s: &OverlaySettings) -> String {
    let s = s.sanitized();
    let keys = mango_keys(&s.items);
    let mut out = String::new();
    out.push_str("# Gerado pelo RESET EXPLODE (tela Overlay). Mudanças feitas à mão aqui são substituídas quando você salva\n");
    out.push_str("# pelo app; se já havia um arquivo aqui, a cópia dele está em MangoHud.conf.antes-do-reset-explode.\n");
    out.push_str(&format!("{MARKER} items={};corner={:?};size={};toggle={};hidden={}\n\n", s.items.join(","), s.corner, s.size.name(), s.toggle, s.start_hidden as u8));

    out.push_str("# só o que foi escolhido, na ordem abaixo\nlegacy_layout=0\n");
    out.push_str(&format!("position={}\nfont_size={}\n", s.corner.mango(), s.size.font_size()));
    out.push_str(&format!("toggle_hud={}\n", s.toggle));
    if s.start_hidden {
        out.push_str("no_display=1\n");
    }
    // mesma cara do overlay do Windows: fundo preto translúcido, texto claro, rótulos em cinza
    let row = |list: &[&str]| keys.iter().filter(|k| list.contains(k)).count();
    let columns = (row(GPU_ROW).max(row(CPU_ROW)) + 1).clamp(3, 5);
    out.push_str(&format!(
        "background_color=000000\nbackground_alpha=0.6\nround_corners=6\ntable_columns={columns}\n\
         text_color=F5F5F5\ngpu_color=8C8C8C\ncpu_color=8C8C8C\nvram_color=8C8C8C\nram_color=8C8C8C\n\
         engine_color=8C8C8C\nframetime_color=F5F5F5\n\n"
    ));

    if keys.is_empty() {
        out.push_str("# nenhum item escolhido: o overlay fica vazio\n");
    }
    for k in &keys {
        match *k {
            "fps_metrics" => out.push_str("fps_metrics=0.01\n"),
            "time" => out.push_str("time\ntime_format=%H:%M\n"),
            k => {
                out.push_str(k);
                out.push('\n');
            }
        }
    }
    // o frametime vem ligado por padrão no bloco do FPS: desliga se não foi escolhido
    if keys.contains(&"fps") && !keys.contains(&"frametime") {
        out.push_str("frametime=0\n");
    }
    out
}

/// Lê as escolhas da linha `# reset-explode:` de um arquivo gerado pelo app. None se o arquivo não é nosso.
pub fn parse(text: &str) -> Option<OverlaySettings> {
    let line = text.lines().find_map(|l| l.strip_prefix(MARKER))?;
    let mut s = OverlaySettings { items: Vec::new(), ..Default::default() };
    for part in line.trim().split(';') {
        let (k, v) = part.split_once('=')?;
        match k {
            "items" => s.items = v.split(',').filter(|x| !x.is_empty()).map(String::from).collect(),
            "corner" => s.corner = Corner::parse(v)?,
            "size" => s.size = Size::parse(v)?,
            "toggle" => s.toggle = v.to_string(),
            "hidden" => s.start_hidden = v == "1",
            _ => {}
        }
    }
    Some(s.sanitized())
}

/// O arquivo é exatamente o que o app gerou (ninguém mexeu à mão depois).
fn is_untouched(text: &str) -> bool {
    parse(text).is_some_and(|s| generate(&s) == text)
}

// ------------------------------------------------------------------ arquivos

#[derive(Debug, Clone, Serialize)]
pub struct Paths {
    pub conf: PathBuf,
    pub backup: PathBuf,
    pub env_file: PathBuf,
}

impl Paths {
    /// `dir` é a pasta de configuração do usuário (normalmente `~/.config`).
    pub fn in_config_dir(dir: &Path) -> Self {
        let conf = dir.join("MangoHud").join("MangoHud.conf");
        Self {
            backup: conf.with_file_name("MangoHud.conf.antes-do-reset-explode"),
            conf,
            env_file: dir.join("environment.d").join("90-reset-explode-mangohud.conf"),
        }
    }

    /// `$XDG_CONFIG_HOME` ou `~/.config`.
    pub fn user() -> Option<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(Self::in_config_dir(&base))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfState {
    pub exists: bool,
    /// Gerado pelo app.
    pub ours: bool,
    /// Gerado pelo app e alterado à mão depois (salvar de novo guarda uma cópia antes).
    pub edited: bool,
    pub backup_exists: bool,
    /// Escolhas salvas (ou o padrão, se o arquivo não é do app).
    pub settings: OverlaySettings,
    pub text: Option<String>,
}

pub fn read_state(p: &Paths) -> ConfState {
    let text = fs::read_to_string(&p.conf).ok();
    let parsed = text.as_deref().and_then(parse);
    ConfState {
        exists: text.is_some(),
        ours: parsed.is_some(),
        edited: parsed.is_some() && !text.as_deref().is_some_and(is_untouched),
        backup_exists: p.backup.exists(),
        settings: parsed.unwrap_or_default(),
        text,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SaveResult {
    /// Para onde foi a cópia do arquivo anterior, se precisou de uma.
    pub backup: Option<PathBuf>,
}

/// Grava o `MangoHud.conf`. Antes, guarda uma cópia do arquivo existente, a não ser que ele seja exatamente o que
/// o app gerou da última vez. A primeira cópia fica com o nome fixo (é a que "Restaurar original" usa); as
/// seguintes ganham um número, para nada se perder.
pub fn save(p: &Paths, s: &OverlaySettings) -> Result<SaveResult, String> {
    let dir = p.conf.parent().ok_or("caminho inválido")?;
    fs::create_dir_all(dir).map_err(|e| format!("não deu para criar {}: {e}", dir.display()))?;
    let mut backup = None;
    if let Ok(old) = fs::read_to_string(&p.conf) {
        if !is_untouched(&old) {
            let target = free_backup_name(&p.backup);
            fs::copy(&p.conf, &target).map_err(|e| format!("não deu para guardar a cópia do arquivo antigo: {e}"))?;
            backup = Some(target);
        }
    }
    write_atomic(&p.conf, &generate(s))?;
    Ok(SaveResult { backup })
}

fn free_backup_name(base: &Path) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    (2..)
        .map(|n| PathBuf::from(format!("{}.{n}", base.display())))
        .find(|p| !p.exists())
        .unwrap()
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp-reset-explode");
    fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, path)).map_err(|e| format!("não deu para gravar {}: {e}", path.display()))
}

/// Desfaz: volta o arquivo que existia antes do app (se havia um) ou apaga o arquivo gerado pelo app.
/// Um arquivo que não é do app nunca é apagado.
pub fn restore(p: &Paths) -> Result<String, String> {
    if p.backup.exists() {
        fs::rename(&p.backup, &p.conf).map_err(|e| format!("não deu para restaurar a cópia: {e}"))?;
        return Ok("Seu MangoHud.conf anterior voltou.".into());
    }
    match fs::read_to_string(&p.conf) {
        Ok(t) if parse(&t).is_some() => {
            fs::remove_file(&p.conf).map_err(|e| format!("não deu para apagar o arquivo: {e}"))?;
            Ok("Arquivo do app removido: o MangoHud volta ao visual padrão dele.".into())
        }
        Ok(_) => Err("o MangoHud.conf atual não foi gerado pelo app; nada foi mexido.".into()),
        Err(_) => Ok("Não há MangoHud.conf: o MangoHud já usa o visual padrão.".into()),
    }
}

const ENV_TEXT: &str = "# Criado pelo RESET EXPLODE (tela Overlay): liga o MangoHud em todos os jogos Vulkan.\n\
# Vale a partir do próximo login. Para desfazer, desligue no app ou apague este arquivo.\n\
MANGOHUD=1\n";

#[derive(Debug, Clone, Serialize)]
pub struct GlobalState {
    /// O arquivo do app existe (vale a partir do próximo login).
    pub enabled: bool,
    /// A sessão atual já tem MANGOHUD=1 (deste arquivo ou de outro lugar).
    pub active_now: bool,
    pub path: PathBuf,
}

pub fn global_state(p: &Paths) -> GlobalState {
    GlobalState {
        enabled: fs::read_to_string(&p.env_file).is_ok_and(|t| t.lines().any(|l| l.trim() == "MANGOHUD=1")),
        active_now: std::env::var("MANGOHUD").is_ok_and(|v| v == "1"),
        path: p.env_file.clone(),
    }
}

/// Liga/desliga `MANGOHUD=1` para a sessão inteira (systemd environment.d do usuário).
pub fn set_global(p: &Paths, on: bool) -> Result<(), String> {
    if on {
        let dir = p.env_file.parent().ok_or("caminho inválido")?;
        fs::create_dir_all(dir).map_err(|e| format!("não deu para criar {}: {e}", dir.display()))?;
        write_atomic(&p.env_file, ENV_TEXT)
    } else if p.env_file.exists() {
        fs::remove_file(&p.env_file).map_err(|e| format!("não deu para apagar {}: {e}", p.env_file.display()))
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ instalação

#[derive(Debug, Clone, Serialize)]
pub struct Install {
    pub installed: bool,
    pub version: Option<String>,
    /// Versão de 32 bits (jogos antigos e alguns do Proton).
    pub lib32: bool,
    /// Repositório multilib ligado (dá para instalar a de 32 bits).
    pub multilib: bool,
    /// O que o botão "Instalar" pede ao pacman.
    pub packages: Vec<String>,
    /// vkcube (vulkan-tools) existe: dá para testar o overlay sem abrir um jogo.
    pub can_test: bool,
}

pub fn install_state() -> Install {
    let q = |pkg: &str| {
        let out = Command::new("pacman").args(["-Q", pkg]).env("LC_ALL", "C").output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).to_string();
        out.status.success().then(|| s.split_whitespace().nth(1).unwrap_or("").to_string())
    };
    let version = q("mangohud");
    let installed = version.is_some() || Path::new("/usr/bin/mangohud").exists();
    let lib32 = q("lib32-mangohud").is_some();
    let multilib = fs::read_to_string("/etc/pacman.conf").is_ok_and(|t| multilib_enabled(&t));
    let mut packages = Vec::new();
    if !installed {
        packages.push("mangohud".to_string());
    }
    if multilib && !lib32 {
        packages.push("lib32-mangohud".to_string());
    }
    Install { installed, version, lib32, multilib, packages, can_test: installed && Path::new("/usr/bin/vkcube").exists() }
}

fn multilib_enabled(pacman_conf: &str) -> bool {
    pacman_conf.lines().any(|l| l.trim() == "[multilib]")
}

/// Abre o vkcube com o MangoHud, para ver o overlay sem abrir um jogo.
pub fn launch_test() -> Result<(), String> {
    Command::new("/usr/bin/mangohud")
        .arg("/usr/bin/vkcube")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("não deu para abrir o teste: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("re-overlay-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn set(items: &[&str]) -> OverlaySettings {
        OverlaySettings { items: items.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    fn body(text: &str) -> Vec<&str> {
        text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect()
    }

    #[test]
    fn catalogo_coerente() {
        for m in METRICS {
            assert!(!m.mango.is_empty(), "{}", m.key);
            assert!(m.mango.iter().all(|k| ORDER.contains(k)), "{} usa chave fora da ordem", m.key);
        }
        let mut keys: Vec<_> = METRICS.iter().map(|m| m.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), METRICS.len());
    }

    #[test]
    fn so_fps() {
        let t = generate(&set(&["fps", "frametime", "low1"]));
        let b = body(&t);
        assert!(b.contains(&"legacy_layout=0"));
        assert!(b.contains(&"fps") && b.contains(&"frametime") && b.contains(&"fps_metrics=0.01"));
        assert!(!b.iter().any(|l| ["gpu_stats", "cpu_stats", "vram", "ram", "frame_timing"].contains(l)));
        assert!(!b.contains(&"frametime=0"));
    }

    #[test]
    fn itens_de_gpu_ligam_a_linha_da_gpu() {
        let b = generate(&set(&["gpu_temp", "gpu_hotspot"]));
        let b = body(&b);
        assert!(b.contains(&"gpu_stats") && b.contains(&"gpu_temp") && b.contains(&"gpu_junction_temp"));
        // o bloco vem antes das chaves que só ligam valores dentro dele
        let pos = |k: &str| b.iter().position(|l| *l == k).unwrap();
        assert!(pos("gpu_stats") < pos("gpu_temp"));
    }

    #[test]
    fn sem_repetir_e_ordem_fixa() {
        let keys = mango_keys(&preset("all"));
        let mut sorted = keys.clone();
        sorted.dedup();
        assert_eq!(keys, sorted);
        let mut uniq = keys.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), keys.len());
        assert_eq!(keys.first(), Some(&"fps"));
        assert_eq!(keys.last(), Some(&"time"));
    }

    #[test]
    fn fps_sem_frametime_desliga_o_padrao() {
        let t = generate(&set(&["fps"]));
        assert!(body(&t).contains(&"frametime=0"));
        let t = generate(&set(&["cpu_load"]));
        assert!(!t.contains("frametime=0") && !body(&t).contains(&"fps"));
    }

    #[test]
    fn posicao_tamanho_atalho_hora() {
        let s = OverlaySettings { corner: Corner::BR, size: Size::Large, toggle: "F12".into(), start_hidden: true, ..set(&["clock"]) };
        let b = generate(&s);
        let b = body(&b);
        for l in ["position=bottom-right", "font_size=31", "toggle_hud=F12", "no_display=1", "time", "time_format=%H:%M"] {
            assert!(b.contains(&l), "faltou {l}");
        }
    }

    #[test]
    fn atalho_invalido_vira_padrao() {
        let s = OverlaySettings { toggle: "rm -rf".into(), ..Default::default() };
        assert!(generate(&s).contains("toggle_hud=Shift_R+F12\n"));
    }

    #[test]
    fn ida_e_volta() {
        let s = OverlaySettings { corner: Corner::TR, size: Size::Small, toggle: "Shift_L+F1".into(), start_hidden: true, ..set(&["gpu_fan", "fps", "lixo"]) };
        let back = parse(&generate(&s)).unwrap();
        assert_eq!(back.items, vec!["fps", "gpu_fan"]); // ordem do catálogo, sem o desconhecido
        assert_eq!((back.corner, back.size, back.toggle.as_str(), back.start_hidden), (Corner::TR, Size::Small, "Shift_L+F1", true));
        assert!(is_untouched(&generate(&back)));
        assert!(parse("fps\ngpu_stats\n").is_none());
    }

    #[test]
    fn colunas_crescem_com_a_linha_da_gpu() {
        assert!(generate(&set(&["fps"])).contains("table_columns=3\n"));
        assert!(generate(&OverlaySettings { items: preset("all"), ..Default::default() }).contains("table_columns=5\n"));
    }

    #[test]
    fn salvar_guarda_copia_e_restaurar_volta() {
        let dir = tmp("save");
        let p = Paths::in_config_dir(&dir);
        fs::create_dir_all(p.conf.parent().unwrap()).unwrap();
        fs::write(&p.conf, "fps\nposition=top-right\n").unwrap();

        // 1ª vez: o arquivo do usuário vai para a cópia
        let r = save(&p, &OverlaySettings::default()).unwrap();
        assert_eq!(r.backup.as_deref(), Some(p.backup.as_path()));
        assert_eq!(fs::read_to_string(&p.backup).unwrap(), "fps\nposition=top-right\n");
        let st = read_state(&p);
        assert!(st.ours && !st.edited && st.backup_exists);

        // salvar de novo um arquivo nosso intacto não faz cópia
        assert!(save(&p, &set(&["fps"])).unwrap().backup.is_none());

        // mexido à mão: ganha cópia numerada (a primeira não é sobrescrita)
        let t = fs::read_to_string(&p.conf).unwrap() + "font_size=40\n";
        fs::write(&p.conf, t).unwrap();
        assert!(read_state(&p).edited);
        let r = save(&p, &set(&["fps"])).unwrap();
        assert_eq!(r.backup.unwrap(), PathBuf::from(format!("{}.2", p.backup.display())));
        assert_eq!(fs::read_to_string(&p.backup).unwrap(), "fps\nposition=top-right\n");

        // restaurar devolve o original
        restore(&p).unwrap();
        assert_eq!(fs::read_to_string(&p.conf).unwrap(), "fps\nposition=top-right\n");
        assert!(!p.backup.exists());
        // sem cópia e com arquivo que não é nosso: não mexe
        assert!(restore(&p).is_err());
        assert!(p.conf.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn restaurar_sem_copia_apaga_so_o_nosso() {
        let dir = tmp("restore");
        let p = Paths::in_config_dir(&dir);
        save(&p, &OverlaySettings::default()).unwrap();
        assert!(!p.backup.exists());
        restore(&p).unwrap();
        assert!(!p.conf.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn global_liga_e_desliga() {
        let dir = tmp("global");
        let p = Paths::in_config_dir(&dir);
        assert!(!global_state(&p).enabled);
        set_global(&p, true).unwrap();
        assert!(global_state(&p).enabled);
        assert!(fs::read_to_string(&p.env_file).unwrap().contains("\nMANGOHUD=1\n"));
        set_global(&p, false).unwrap();
        assert!(!p.env_file.exists());
        set_global(&p, false).unwrap(); // desligar de novo não dá erro
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn multilib() {
        assert!(multilib_enabled("[core]\nInclude = x\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n"));
        assert!(!multilib_enabled("#[multilib]\n#Include = /etc/pacman.d/mirrorlist\n"));
    }
}
