//! Aplicativos instalados e catálogo para instalar (equivalente ao "Instalar apps" do Windows, que usava o winget).
//! "Instalado" aqui é o que aparece no menu: os atalhos .desktop, ligados ao pacote do pacman que os trouxe.

use crate::packages::installed_versions;
use crate::util::human_bytes;
use base64::Engine;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct App {
    /// Pacote (pacman) que instalou o app; vazio se foi instalado à mão.
    pub package: String,
    pub name: String,
    pub comment: String,
    /// Ícone como data URL (PNG/SVG), ou vazio.
    pub icon: String,
    /// "Repositório", "AUR" ou "Manual"
    pub source: String,
    pub version: String,
    pub size: String,
    /// Outros apps do mesmo pacote (LibreOffice traz Writer, Calc...).
    pub also: Vec<String>,
    /// Por que não dá para remover pelo app (vazio = pode).
    pub locked: String,
}

/// Peças do sistema: remover qualquer uma derruba a sessão gráfica ou o próprio gerenciador de pacotes.
const PROTECTED: &[&str] = &[
    "pacman", "systemd", "glibc", "bash", "sudo", "polkit", "dbus", "gnome-shell", "gnome-session", "gdm", "mutter",
    "gnome-control-center", "gnome-settings-daemon", "nautilus", "networkmanager", "pipewire", "wireplumber",
    "xdg-desktop-portal", "xdg-desktop-portal-gnome", "gnome-terminal", "kitty", "nvidia-settings", "nvidia-utils",
];

fn protected(pkg: &str) -> bool {
    PROTECTED.contains(&pkg) || pkg.starts_with("cachyos-") || pkg.starts_with("linux")
}

struct Desktop {
    path: PathBuf,
    name: String,
    comment: String,
    icon: String,
}

/// Lê um .desktop. Ignora os escondidos (NoDisplay/Hidden) e o que não é aplicativo.
fn parse_desktop(path: &Path) -> Option<Desktop> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut f: HashMap<&str, &str> = HashMap::new();
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some((k, v)) = l.split_once('=') {
                f.entry(k.trim()).or_insert(v.trim());
            }
        }
    }
    if f.get("Type") != Some(&"Application") || f.get("NoDisplay") == Some(&"true") || f.get("Hidden") == Some(&"true") {
        return None;
    }
    let local = |k: &str| f.get(format!("{k}[pt_BR]").as_str()).or(f.get(format!("{k}[pt]").as_str())).or(f.get(k)).map(|s| s.to_string());
    Some(Desktop { path: path.to_path_buf(), name: local("Name")?, comment: local("Comment").unwrap_or_default(), icon: f.get("Icon").unwrap_or(&"").to_string() })
}

fn desktop_files() -> Vec<Desktop> {
    let mut dirs = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/usr/local/share/applications")];
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/applications"));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        files.sort();
        for p in files {
            // o mesmo id em ~/.local sobrepõe o do sistema; aqui vale o primeiro, então o do sistema
            let id = p.file_name().unwrap_or_default().to_os_string();
            if seen.insert(id) {
                out.extend(parse_desktop(&p));
            }
        }
    }
    out
}

/// arquivo -> pacote dono, numa chamada só do pacman.
fn owners(files: &[PathBuf]) -> HashMap<PathBuf, String> {
    let out = Command::new("pacman").arg("-Qo").args(files).env("LC_ALL", "C").output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (path, rest) = l.split_once(" is owned by ")?;
            Some((PathBuf::from(path), rest.split_whitespace().next()?.to_string()))
        })
        .collect()
}

struct PkgInfo {
    size: String,
    required_by: Vec<String>,
}

/// `pacman -Qi`: tamanho e quem depende de cada pacote.
fn pkg_info(pkgs: &[String]) -> HashMap<String, PkgInfo> {
    let mut m = HashMap::new();
    if pkgs.is_empty() {
        return m;
    }
    let out = Command::new("pacman").arg("-Qi").args(pkgs).env("LC_ALL", "C").output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    for block in text.split("\n\n") {
        let field = |k: &str| {
            block
                .lines()
                .find(|l| l.starts_with(k))
                .and_then(|l| l.split_once(" : "))
                .map(|(_, v)| v.trim().to_string())
        };
        let Some(name) = field("Name ") else { continue };
        let required_by = field("Required By").filter(|v| v != "None").map(|v| v.split_whitespace().map(String::from).collect()).unwrap_or_default();
        let size = field("Installed Size").map(|v| size_text(&v)).unwrap_or_default();
        m.insert(name, PkgInfo { size, required_by });
    }
    m
}

/// "1737.27 KiB" -> "1,7 MB"
fn size_text(v: &str) -> String {
    let mut it = v.split_whitespace();
    let (Some(n), Some(unit)) = (it.next().and_then(|n| n.parse::<f64>().ok()), it.next()) else { return v.to_string() };
    let mult = match unit {
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    human_bytes((n * mult) as u64)
}

fn foreign() -> HashSet<String> {
    let out = Command::new("pacman").arg("-Qqm").output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    text.lines().map(String::from).collect()
}

pub fn installed() -> Vec<App> {
    let desk = desktop_files();
    let paths: Vec<PathBuf> = desk.iter().map(|d| d.path.clone()).collect();
    let own = owners(&paths);
    let versions = installed_versions();
    let aur = foreign();
    let icons = IconIndex::build();

    // um item por pacote; o primeiro atalho dá o nome, os outros viram "também traz"
    let mut by_pkg: Vec<(String, Vec<&Desktop>)> = Vec::new();
    for d in &desk {
        let pkg = own.get(&d.path).cloned().unwrap_or_default();
        match by_pkg.iter_mut().find(|(p, _)| !pkg.is_empty() && *p == pkg) {
            Some((_, list)) => list.push(d),
            None => by_pkg.push((pkg, vec![d])),
        }
    }
    let pkgs: Vec<String> = by_pkg.iter().map(|(p, _)| p.clone()).filter(|p| !p.is_empty()).collect();
    let info = pkg_info(&pkgs);

    let mut apps: Vec<App> = by_pkg
        .into_iter()
        .map(|(pkg, list)| {
            let main = list[0];
            let inf = info.get(&pkg);
            let locked = if pkg.is_empty() {
                "instalado fora do gerenciador de pacotes".to_string()
            } else if protected(&pkg) {
                "faz parte do sistema".to_string()
            } else if let Some(req) = inf.filter(|i| !i.required_by.is_empty()) {
                format!("outros pacotes dependem dele: {}", req.required_by.join(", "))
            } else {
                String::new()
            };
            App {
                name: main.name.clone(),
                comment: main.comment.clone(),
                icon: icons.data_url(&main.icon),
                source: if pkg.is_empty() { "Manual" } else if aur.contains(&pkg) { "AUR" } else { "Repositório" }.into(),
                version: versions.get(&pkg).cloned().unwrap_or_default(),
                size: inf.map(|i| i.size.clone()).unwrap_or_default(),
                also: list[1..].iter().map(|d| d.name.clone()).collect(),
                locked,
                package: pkg,
            }
        })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

// ---------------------------------------------------------------- ícones

/// Nome do ícone -> arquivo, procurando no tema do usuário (e nos que ele herda), no hicolor e em pixmaps.
struct IconIndex {
    files: HashMap<String, PathBuf>,
}

impl IconIndex {
    fn build() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let roots = [home.join(".local/share/icons"), home.join(".icons"), PathBuf::from("/usr/share/icons")];
        let mut themes = Vec::new();
        let mut next = vec![current_icon_theme()];
        // segue o "Inherits" do index.theme
        while let Some(t) = next.pop() {
            if t.is_empty() || themes.contains(&t) || themes.len() > 6 {
                continue;
            }
            for r in &roots {
                let idx = r.join(&t).join("index.theme");
                if let Ok(text) = fs::read_to_string(idx) {
                    if let Some(inh) = text.lines().find_map(|l| l.strip_prefix("Inherits=")) {
                        next.extend(inh.split(',').map(|s| s.trim().to_string()));
                    }
                    break;
                }
            }
            themes.push(t);
        }
        for t in ["hicolor", "Adwaita"] {
            if !themes.iter().any(|x| x == t) {
                themes.push(t.into());
            }
        }

        let mut files: HashMap<String, (u32, PathBuf)> = HashMap::new();
        // ordem de preferência: tema do usuário primeiro; dentro dele, SVG, depois PNG maior (até 256)
        for (rank, t) in themes.iter().enumerate() {
            for r in &roots {
                walk(&r.join(t), 0, &mut |p| {
                    let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return };
                    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                    let s = p.to_string_lossy();
                    if !s.contains("/apps") {
                        return;
                    }
                    let quality = match ext.as_str() {
                        "svg" => 300,
                        "png" => ["256", "128", "96", "64", "48", "32"].iter().position(|z| s.contains(&format!("/{z}x{z}")) || s.contains(&format!("/{z}/"))).map_or(10, |i| 200 - i as u32 * 10),
                        _ => return,
                    };
                    let score = (100 - rank as u32) * 1000 + quality;
                    if files.get(&stem).is_none_or(|(old, _)| score > *old) {
                        files.insert(stem, (score, p.to_path_buf()));
                    }
                });
            }
        }
        if let Ok(rd) = fs::read_dir("/usr/share/pixmaps") {
            for e in rd.flatten() {
                let p = e.path();
                if let (Some(stem), Some(ext)) = (p.file_stem(), p.extension()) {
                    if ext == "png" || ext == "svg" {
                        files.entry(stem.to_string_lossy().into_owned()).or_insert((0, p.clone()));
                    }
                }
            }
        }
        IconIndex { files: files.into_iter().map(|(k, (_, p))| (k, p)).collect() }
    }

    fn data_url(&self, icon: &str) -> String {
        let path = if icon.starts_with('/') { Some(PathBuf::from(icon)) } else { self.files.get(icon).cloned() };
        let Some(path) = path else { return String::new() };
        let mime = match path.extension().and_then(|e| e.to_str()) {
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            _ => return String::new(),
        };
        match fs::read(&path) {
            // ícones enormes (alguns SVG passam de 1 MB; um PNG 512x512 chega a ~500 KB) deixariam a lista pesada: fica a inicial no lugar
            Ok(b) if b.len() <= 1024 * 1024 => format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(b)),
            _ => String::new(),
        }
    }
}

fn walk(dir: &Path, depth: u32, f: &mut dyn FnMut(&Path)) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        // temas usam links simbólicos até para pastas inteiras (no MacTahoe-dark, "apps/scalable" aponta
        // para o MacTahoe): segue o link; o limite de profundidade evita laços como "apps@2x -> apps"
        let Ok(meta) = fs::metadata(&p) else { continue };
        if meta.is_dir() {
            // ícones "symbolic" são monocromáticos, para barras e menus: não servem de ícone de app
            if e.file_name() != "symbolic" {
                walk(&p, depth + 1, f);
            }
        } else if meta.is_file() {
            f(&p);
        }
    }
}

fn current_icon_theme() -> String {
    Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- catálogo

#[derive(Debug, Clone, Serialize)]
pub struct CatalogApp {
    pub package: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    /// No Windows era este; aqui é o equivalente.
    pub replaces: &'static str,
    pub installed: bool,
    /// Ícone do tema do usuário (data URL); temas como o MacTahoe trazem ícones até de apps não instalados.
    pub icon: String,
}

/// Pacote, nome, descrição, categoria, o que substitui do Windows e nomes de ícone (o primeiro que existir).
/// Os mesmos do app do Windows (Steam, Chrome, Discord, Lightshot, 7-Zip, WinRAR, VLC) com o equivalente
/// nos repositórios do Arch/CachyOS, mais o que costuma fazer falta depois de formatar.
const CATALOG: &[(&str, &str, &str, &str, &str, &str)] = &[
    ("steam", "Steam", "Loja e biblioteca de jogos, com Proton para jogos de Windows", "Jogos", "", "steam"),
    ("heroic-games-launcher-bin", "Heroic", "Epic Games, GOG e Amazon Games", "Jogos", "", "com.heroicgameslauncher.hgl|heroic"),
    ("lutris", "Lutris", "Jogos de outras lojas e emuladores", "Jogos", "", "net.lutris.Lutris|lutris"),
    ("protonup-qt", "ProtonUp-Qt", "Instala o Proton-GE e outras versões do Proton", "Jogos", "", "net.davidotek.pupgui2|protonup-qt"),
    ("mangohud", "MangoHud", "Overlay de FPS e sensores dentro do jogo", "Jogos", "", "mangohud|utilities-system-monitor"),
    ("gamemode", "GameMode", "Ajusta CPU e prioridade enquanto um jogo roda", "Jogos", "", "gamemode|applications-games|input-gaming"),
    ("discord", "Discord", "Voz e chat", "Internet", "", "discord"),
    ("chromium", "Chromium", "A base aberta do Google Chrome", "Internet", "Google Chrome", "chromium"),
    ("firefox", "Firefox", "Navegador", "Internet", "", "firefox"),
    ("telegram-desktop", "Telegram", "Mensagens", "Internet", "", "org.telegram.desktop|telegram"),
    ("qbittorrent", "qBittorrent", "Cliente de torrent", "Internet", "", "org.qbittorrent.qBittorrent|qbittorrent"),
    ("vlc", "VLC", "Toca qualquer vídeo ou áudio", "Multimídia", "", "vlc"),
    ("spotify-launcher", "Spotify", "Música", "Multimídia", "", "spotify-launcher|spotify|spotify-client"),
    ("obs-studio", "OBS Studio", "Gravação e transmissão de tela", "Multimídia", "", "com.obsproject.Studio|obs"),
    ("gimp", "GIMP", "Edição de imagens", "Multimídia", "", "org.gimp.GIMP|gimp"),
    ("flameshot", "Flameshot", "Captura de tela com anotações", "Utilitários", "Lightshot", "org.flameshot.Flameshot|flameshot"),
    ("7zip", "7-Zip", "Compacta e extrai .7z, .zip e outros", "Utilitários", "7-Zip", "7zip|p7zip"),
    ("unrar", "unrar", "Extrai arquivos .rar (o gerenciador de arquivos passa a abrir .rar)", "Utilitários", "WinRAR", "org.gnome.FileRoller|file-roller|ark"),
    ("libreoffice-fresh", "LibreOffice", "Textos, planilhas e apresentações", "Produtividade", "Microsoft Office", "libreoffice-startcenter|libreoffice-main"),
    ("code", "Code - OSS", "Editor de código (base aberta do VS Code)", "Produtividade", "VS Code", "code-oss|code|vscode"),
];

pub fn catalog() -> Vec<CatalogApp> {
    let q = installed_versions();
    let icons = IconIndex::build();
    CATALOG
        .iter()
        .map(|&(package, name, description, category, replaces, icon)| CatalogApp {
            package,
            name,
            description,
            category,
            replaces,
            installed: q.contains_key(package),
            icon: icon.split('|').map(|n| icons.data_url(n)).find(|u| !u.is_empty()).unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_em_portugues_e_escondidos() {
        let dir = std::env::temp_dir().join(format!("re-desktop-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.desktop");
        fs::write(&a, "[Desktop Entry]\nType=Application\nName=Files\nName[pt_BR]=Arquivos\nIcon=org.gnome.Nautilus\n[Desktop Action new]\nName=Nova janela\n").unwrap();
        let b = dir.join("b.desktop");
        fs::write(&b, "[Desktop Entry]\nType=Application\nName=Oculto\nNoDisplay=true\n").unwrap();
        let d = parse_desktop(&a).unwrap();
        assert_eq!((d.name.as_str(), d.icon.as_str()), ("Arquivos", "org.gnome.Nautilus"));
        assert!(parse_desktop(&b).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn catalogo_so_tem_nomes_validos() {
        assert!(CATALOG.iter().all(|c| crate::packages::valid_package(c.0)));
    }
}
