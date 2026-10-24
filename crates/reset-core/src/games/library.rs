//! Jogos instalados: Steam (nativos e Proton, em todas as bibliotecas), Heroic (Epic, GOG, Amazon e adicionados à
//! mão) e Lutris. Portado do GameLibrary.cs. SÓ LÊ: nenhum arquivo da Steam/Heroic/Lutris é alterado (o
//! localconfig.vdf, por exemplo, é sobrescrito pela Steam aberta e corromperia com escrita de fora).

use super::vdf::{self, Node};
use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize)]
pub struct Game {
    /// "steam:1174180", "heroic:legendary:Fortnite", "lutris:the-witcher-3"
    pub key: String,
    /// "Steam", "Heroic" ou "Lutris"
    pub store: String,
    pub id: String,
    pub name: String,
    pub install_dir: String,
    pub size_bytes: Option<u64>,
    /// Unix, segundos.
    pub last_played: Option<u64>,
    pub playtime_min: Option<u64>,
    /// "Proton", "Nativo", "Wine"...
    pub platform: String,
    /// De onde veio, por extenso: "Steam · /mnt/jogos/SteamLibrary", "Epic Games (Heroic)"...
    pub source: String,
    /// Steam: há atualização pendente.
    pub update_pending: bool,
    /// Steam: as "Opções de inicialização" que estão gravadas agora (só leitura).
    pub launch_options: Option<String>,
}

impl Game {
    pub fn is_steam(&self) -> bool {
        self.store == "Steam"
    }
    pub fn proton(&self) -> bool {
        matches!(self.platform.as_str(), "Proton" | "Wine")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    pub games: Vec<Game>,
    /// Lojas encontradas neste PC (mesmo sem jogos).
    pub stores: Vec<String>,
}

pub fn scan() -> Library {
    let mut games = Vec::new();
    let mut stores = Vec::new();
    let steam = steam_roots();
    if !steam.is_empty() {
        stores.push("Steam".to_string());
        games.extend(steam_games(&steam));
    }
    let heroic = heroic_bases();
    if !heroic.is_empty() {
        stores.push("Heroic".to_string());
        for b in &heroic {
            games.extend(heroic_games(b));
        }
    }
    if let Some(db) = lutris_db() {
        stores.push("Lutris".to_string());
        games.extend(lutris_games(&db));
    }
    // o mesmo jogo pode aparecer duas vezes (Steam nativo + Flatpak apontando para a mesma pasta)
    let mut seen = std::collections::HashSet::new();
    games.retain(|g| seen.insert(g.key.clone()));
    games.sort_by_key(|g| g.name.to_lowercase());
    Library { games, stores }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn read_vdf(p: &Path) -> Option<Node> {
    let bytes = fs::read(p).ok()?;
    Some(vdf::parse(&String::from_utf8_lossy(&bytes)))
}

// ---------------------------------------------------------------- Steam

/// Instalações da Steam: nativa, o link ~/.steam/steam, Flatpak e Snap. Cada uma aparece uma vez só.
fn steam_roots() -> Vec<PathBuf> {
    let h = home();
    let cands = [
        h.join(".local/share/Steam"),
        h.join(".steam/steam"),
        h.join(".steam/root"),
        h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        h.join("snap/steam/common/.local/share/Steam"),
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for c in cands {
        let Ok(real) = fs::canonicalize(&c) else { continue };
        if real.join("steamapps").is_dir() && !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

/// Pastas de biblioteca (a da instalação + as do libraryfolders.vdf).
fn steam_libraries(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    for f in [root.join("steamapps/libraryfolders.vdf"), root.join("config/libraryfolders.vdf")] {
        let Some(v) = read_vdf(&f) else { continue };
        for p in library_paths(&v) {
            let p = fs::canonicalize(&p).unwrap_or(p);
            if p.join("steamapps").is_dir() && !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

fn library_paths(v: &Node) -> Vec<PathBuf> {
    let Some(lf) = v.child("libraryfolders") else { return Vec::new() };
    lf.items
        .iter()
        .filter_map(|(_, val)| match val {
            vdf::Value::Node(n) => n.get("path").map(PathBuf::from),
            // formato antigo: "1" "/caminho"
            vdf::Value::Str(s) if s.starts_with('/') => Some(PathBuf::from(s)),
            _ => None,
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct Manifest {
    appid: String,
    name: String,
    installdir: String,
    state_flags: u64,
    last_played: Option<u64>,
    size: Option<u64>,
}

fn parse_manifest(text: &str) -> Option<Manifest> {
    let root = vdf::parse(text);
    let st = root.child("AppState")?;
    let appid = st.get("appid")?.to_string();
    if appid.is_empty() || !appid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(Manifest {
        appid,
        name: st.get("name")?.to_string(),
        installdir: st.get("installdir")?.to_string(),
        state_flags: st.get_u64("StateFlags").unwrap_or(4),
        last_played: st.get_u64("LastPlayed").filter(|&t| t > 0),
        size: st.get_u64("SizeOnDisk").filter(|&s| s > 0),
    })
}

/// Ferramentas que a Steam instala como "apps" e não são jogos.
fn is_steam_tool(appid: &str, name: &str) -> bool {
    let l = name.to_lowercase();
    appid == "228980"
        || ["steamworks common", "proton", "steam linux runtime", "steamvr", "steamworks sdk"].iter().any(|p| l.starts_with(p))
}

/// Dados da conta que usou a Steam por último (o localconfig.vdf mais recente).
struct SteamUser {
    /// appid -> nó do app em localconfig (LaunchOptions, Playtime, LastPlayed)
    apps: Node,
    /// appids com plataforma trocada para Windows (compat.vdf)
    windows: Vec<String>,
}

fn steam_user(root: &Path) -> Option<SteamUser> {
    let dirs = fs::read_dir(root.join("userdata")).ok()?;
    let cfg = dirs
        .flatten()
        .map(|d| d.path().join("config"))
        .filter(|c| c.join("localconfig.vdf").is_file())
        .max_by_key(|c| fs::metadata(c.join("localconfig.vdf")).and_then(|m| m.modified()).ok())?;
    let local = read_vdf(&cfg.join("localconfig.vdf")).unwrap_or_default();
    let apps = local.path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]).cloned().unwrap_or_default();
    let windows = read_vdf(&cfg.join("compat.vdf"))
        .and_then(|c| c.child("platform_overrides").cloned())
        .map(|po| po.children().filter(|(_, n)| n.get("src") == Some("windows")).map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default();
    Some(SteamUser { apps, windows })
}

/// appid -> ferramenta de compatibilidade escolhida (config.vdf, CompatToolMapping).
fn compat_tools(root: &Path) -> HashMap<String, String> {
    let Some(cfg) = read_vdf(&root.join("config/config.vdf")) else { return HashMap::new() };
    let Some(map) = cfg.path(&["InstallConfigStore", "Software", "Valve", "Steam", "CompatToolMapping"]) else { return HashMap::new() };
    map.children()
        .filter_map(|(id, n)| Some((id.to_string(), n.get("name").filter(|s| !s.is_empty())?.to_string())))
        .collect()
}

/// Tem .exe na pasta do jogo (até 2 níveis)? Jogos de Windows rodam pelo Proton.
fn has_exe(dir: &Path, depth: u32) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    for e in rd.flatten().take(400) {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
            return true;
        }
        if depth > 0 && e.file_type().is_ok_and(|t| t.is_dir()) && has_exe(&p, depth - 1) {
            return true;
        }
    }
    false
}

fn steam_games(roots: &[PathBuf]) -> Vec<Game> {
    let mut out = Vec::new();
    for root in roots {
        let user = steam_user(root);
        let tools = compat_tools(root);
        for lib in steam_libraries(root) {
            let apps = lib.join("steamapps");
            let Ok(rd) = fs::read_dir(&apps) else { continue };
            for e in rd.flatten() {
                let f = e.file_name().to_string_lossy().into_owned();
                if !(f.starts_with("appmanifest_") && f.ends_with(".acf")) {
                    continue;
                }
                let Some(m) = fs::read(e.path()).ok().and_then(|b| parse_manifest(&String::from_utf8_lossy(&b))) else { continue };
                if m.state_flags & 4 == 0 || is_steam_tool(&m.appid, &m.name) {
                    continue; // 4 = instalado por completo
                }
                let dir = apps.join("common").join(&m.installdir);
                if !dir.is_dir() {
                    continue;
                }
                let local = user.as_ref().and_then(|u| u.apps.child(&m.appid));
                let windows = user.as_ref().is_some_and(|u| u.windows.contains(&m.appid))
                    || tools.contains_key(&m.appid)
                    || apps.join("compatdata").join(&m.appid).is_dir()
                    || has_exe(&dir, 1);
                let last = m.last_played.max(local.and_then(|l| l.get_u64("LastPlayed")).filter(|&t| t > 0));
                out.push(Game {
                    key: format!("steam:{}", m.appid),
                    store: "Steam".into(),
                    id: m.appid.clone(),
                    name: m.name,
                    install_dir: dir.to_string_lossy().into_owned(),
                    size_bytes: m.size,
                    last_played: last,
                    playtime_min: local.and_then(|l| l.get_u64("Playtime")).filter(|&p| p > 0),
                    platform: if windows { "Proton" } else { "Nativo" }.into(),
                    source: format!("Steam · {}", lib.display()),
                    update_pending: m.state_flags & 2 != 0,
                    launch_options: local.and_then(|l| l.get("LaunchOptions")).map(str::to_string),
                });
            }
        }
    }
    out
}

/// Opções de inicialização gravadas agora na Steam para o jogo (lidas de novo, sem cache).
pub fn steam_launch_options(appid: &str) -> Option<String> {
    steam_roots().iter().find_map(|r| steam_user(r)?.apps.child(appid)?.get("LaunchOptions").map(str::to_string))
}

pub fn steam_running() -> bool {
    proc_cmdlines().any(|(_, args)| args.first().is_some_and(|a| a.ends_with("/steam") || a.ends_with("steamwebhelper") || a == "steam"))
}

// ---------------------------------------------------------------- Heroic

fn heroic_bases() -> Vec<PathBuf> {
    let h = home();
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".config"));
    [base.join("heroic"), h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic")].into_iter().filter(|p| p.is_dir()).collect()
}

fn read_json(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

/// Título e capas de cada jogo (app_name -> meta), procurando em todos os caches de biblioteca do Heroic.
#[derive(Default, Clone)]
struct HeroicMeta {
    title: String,
    art_square: String,
    art_cover: String,
}

fn heroic_meta(base: &Path) -> HashMap<String, HeroicMeta> {
    let files = [
        "store_cache/legendary_library.json",
        "store_cache/gog_library.json",
        "store_cache/nile_library.json",
        "gog_store/library.json",
        "sideload_apps/library.json",
    ];
    let mut out = HashMap::new();
    for f in files {
        if let Some(v) = read_json(&base.join(f)) {
            collect_meta(&v, &mut out);
        }
    }
    out
}

fn collect_meta(v: &serde_json::Value, out: &mut HashMap<String, HeroicMeta>) {
    match v {
        serde_json::Value::Object(o) => {
            let s = |k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let app = s("app_name");
            if !app.is_empty() && !s("title").is_empty() {
                out.entry(app).or_insert(HeroicMeta { title: s("title"), art_square: s("art_square"), art_cover: s("art_cover") });
            }
            o.values().for_each(|x| collect_meta(x, out));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_meta(x, out)),
        _ => {}
    }
}

/// "12.5 GB", "1.2 GiB", 123456 → bytes
fn parse_size(v: &serde_json::Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return (n > 0).then_some(n);
    }
    let s = v.as_str()?.trim().replace(',', ".");
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len()));
    let n: f64 = num.trim().parse().ok()?;
    let mult = match unit.trim().to_uppercase().as_str() {
        "KB" | "KIB" => 1024f64,
        "MB" | "MIB" => 1024f64.powi(2),
        "GB" | "GIB" => 1024f64.powi(3),
        "TB" | "TIB" => 1024f64.powi(4),
        _ => 1.0,
    };
    Some((n * mult) as u64).filter(|&b| b > 0)
}

/// "2025-03-01T18:22:05.000Z" → Unix (UTC). Só o que o Heroic grava; nada de fusos.
fn parse_iso(s: &str) -> Option<u64> {
    let d: Vec<i64> = s.get(..19)?.split(|c| "-T:".contains(c)).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [y, mo, da, h, mi, se] = d[..] else { return None };
    // dias desde 1970-01-01 (algoritmo de Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + se).ok()
}

/// O que conta como nome de app seguro para pôr numa URL heroic:// (ids da Epic, GOG e Amazon).
fn safe_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn heroic_games(base: &Path) -> Vec<Game> {
    let meta = heroic_meta(base);
    let stamps = read_json(&base.join("store/timestamp.json"));
    let mut out = Vec::new();
    let mut add = |runner: &str, store: &str, app: &str, dir: &str, size: Option<u64>, title: Option<&str>, platform: &str| {
        if !safe_id(app) {
            return;
        }
        let m = meta.get(app);
        let name = title.filter(|t| !t.is_empty()).map(str::to_string).or(m.map(|m| m.title.clone())).unwrap_or_else(|| app.to_string());
        let ts = stamps.as_ref().and_then(|s| s.get(app));
        out.push(Game {
            key: format!("heroic:{runner}:{app}"),
            store: "Heroic".into(),
            id: app.to_string(),
            name,
            install_dir: dir.to_string(),
            size_bytes: size,
            last_played: ts.and_then(|t| t.get("lastPlayed")).and_then(|v| v.as_str()).and_then(parse_iso),
            playtime_min: ts.and_then(|t| t.get("totalPlayed")).and_then(|v| v.as_u64()).filter(|&m| m > 0),
            platform: if platform.eq_ignore_ascii_case("linux") { "Nativo" } else { "Wine" }.into(),
            source: format!("{store} (Heroic)"),
            update_pending: false,
            launch_options: None,
        });
    };

    // Epic (legendary): { "AppName": { app_name, title, install_path, install_size, platform, is_dlc } }
    for f in ["legendaryConfig/legendary/installed.json"] {
        if let Some(serde_json::Value::Object(o)) = read_json(&base.join(f)) {
            for (app, g) in &o {
                if g.get("is_dlc").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
                let s = |k: &str| g.get(k).and_then(|v| v.as_str()).unwrap_or("");
                add("legendary", "Epic Games", app, s("install_path"), g.get("install_size").and_then(parse_size), Some(s("title")), s("platform"));
            }
        }
    }
    // GOG: { "installed": [ { appName, install_path, install_size, platform } ] }
    if let Some(list) = read_json(&base.join("gog_store/installed.json")).and_then(|v| v.get("installed").cloned()) {
        for g in list.as_array().into_iter().flatten() {
            let s = |k: &str| g.get(k).and_then(|v| v.as_str()).unwrap_or("");
            add("gog", "GOG", s("appName"), s("install_path"), g.get("install_size").and_then(parse_size), None, s("platform"));
        }
    }
    // Amazon (nile): [ { id, path, size } ]
    if let Some(list) = read_json(&base.join("nile_config/nile/installed.json")) {
        for g in list.as_array().into_iter().flatten() {
            let s = |k: &str| g.get(k).and_then(|v| v.as_str()).unwrap_or("");
            add("nile", "Amazon Games", s("id"), s("path"), g.get("size").and_then(parse_size), None, "windows");
        }
    }
    // adicionados à mão: { "games": [ { app_name, title, is_installed, folder_name, install: { platform, executable } } ] }
    if let Some(list) = read_json(&base.join("sideload_apps/library.json")).and_then(|v| v.get("games").cloned()) {
        for g in list.as_array().into_iter().flatten() {
            if g.get("is_installed").and_then(|v| v.as_bool()) == Some(false) {
                continue;
            }
            let s = |k: &str| g.get(k).and_then(|v| v.as_str()).unwrap_or("");
            let plat = g.pointer("/install/platform").and_then(|v| v.as_str()).unwrap_or("");
            add("sideload", "Adicionado", s("app_name"), s("folder_name"), None, Some(s("title")), plat);
        }
    }
    out
}

// ---------------------------------------------------------------- Lutris

fn lutris_data_dirs() -> Vec<PathBuf> {
    let h = home();
    vec![h.join(".local/share/lutris"), h.join(".var/app/net.lutris.Lutris/data/lutris")]
}

fn lutris_db() -> Option<PathBuf> {
    lutris_data_dirs().into_iter().map(|d| d.join("pga.db")).find(|p| p.is_file())
}

fn safe_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// O banco do Lutris é SQLite: lido pelo `sqlite3` em modo imutável (sem travar nem escrever nada).
fn lutris_games(db: &Path) -> Vec<Game> {
    let uri = format!("file:{}?immutable=1", db.display());
    let out = Command::new("sqlite3")
        .args(["-readonly", "-json", &uri, "select id, slug, name, runner, directory, lastplayed, playtime from games where installed = 1"])
        .stdin(Stdio::null())
        .output();
    let Ok(out) = out else { return Vec::new() };
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    rows.iter()
        .filter_map(|r| {
            let s = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let id = r.get("id")?.as_u64()?;
            let slug = s("slug");
            if !safe_slug(&slug) {
                return None;
            }
            let runner = s("runner");
            Some(Game {
                key: format!("lutris:{slug}"),
                store: "Lutris".into(),
                id: id.to_string(),
                name: if s("name").is_empty() { slug.clone() } else { s("name") },
                install_dir: s("directory"),
                size_bytes: None,
                last_played: r.get("lastplayed").and_then(|v| v.as_u64()).filter(|&t| t > 0),
                // o Lutris guarda horas (fração)
                playtime_min: r.get("playtime").and_then(|v| v.as_f64()).map(|h| (h * 60.0).round() as u64).filter(|&m| m > 0),
                platform: match runner.as_str() {
                    "linux" | "steam" => "Nativo".to_string(),
                    "wine" => "Wine".to_string(),
                    "" => "—".to_string(),
                    r => r.to_string(),
                },
                source: format!("Lutris · {runner}"),
                update_pending: false,
                launch_options: None,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- capas e artes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    /// Capa em pé (600x900)
    Cover,
    /// Banner largo do topo da página do jogo
    Hero,
    /// Logo com fundo transparente
    Logo,
}

impl Art {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cover" => Art::Cover,
            "hero" => Art::Hero,
            "logo" => Art::Logo,
            _ => return None,
        })
    }
}

/// A arte do jogo como data URL (a interface não carrega arquivos locais nem imagens da internet).
pub fn art(key: &str, kind: Art) -> Option<String> {
    let (store, rest) = key.split_once(':')?;
    let path = match store {
        "steam" if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) => steam_art(rest, kind),
        "heroic" => rest.split_once(':').filter(|(_, app)| safe_id(app)).and_then(|(_, app)| heroic_art(app, kind)),
        "lutris" if safe_slug(rest) => lutris_art(rest, kind),
        _ => None,
    }?;
    data_url(&path)
}

fn steam_art(appid: &str, kind: Art) -> Option<PathBuf> {
    // nomes por ordem de preferência; o cache novo da Steam guarda em librarycache/<id>/[hash/]nome
    let (custom, names): (&[String], &[&str]) = match kind {
        Art::Cover => (&[format!("{appid}p.png"), format!("{appid}p.jpg")], &["library_600x900.jpg", "library_capsule.jpg", "library_600x900_2x.jpg", "header.jpg", "library_header.jpg"]),
        Art::Hero => (&[format!("{appid}_hero.png"), format!("{appid}_hero.jpg")], &["library_hero.jpg", "library_hero_blur.jpg", "header.jpg", "library_header.jpg"]),
        Art::Logo => (&[format!("{appid}_logo.png")], &["logo.png"]),
    };
    for root in steam_roots() {
        // arte escolhida pelo usuário na Steam (userdata/<conta>/config/grid)
        if let Ok(rd) = fs::read_dir(root.join("userdata")) {
            for u in rd.flatten() {
                if let Some(p) = custom.iter().map(|c| u.path().join("config/grid").join(c)).find(|p| p.is_file()) {
                    return Some(p);
                }
            }
        }
        let cache = root.join("appcache/librarycache");
        let dir = cache.join(appid);
        let mut found: Vec<PathBuf> = Vec::new();
        find_files(&dir, 2, &mut found);
        for n in names {
            if let Some(p) = found.iter().find(|p| p.file_name().is_some_and(|f| f == *n)) {
                return Some(p.clone());
            }
            // formato antigo: librarycache/<id>_library_600x900.jpg
            let old = cache.join(format!("{appid}_{n}"));
            if old.is_file() {
                return Some(old);
            }
        }
    }
    None
}

fn find_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => find_files(&p, depth - 1, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

/// O Heroic guarda as capas baixadas em images-cache/<sha256 da URL>.
fn heroic_art(app: &str, kind: Art) -> Option<PathBuf> {
    for base in heroic_bases() {
        let Some(m) = heroic_meta(&base).remove(app) else { continue };
        let urls = match kind {
            Art::Cover => [m.art_square, m.art_cover],
            Art::Hero | Art::Logo => [m.art_cover, String::new()],
        };
        for u in urls.iter().filter(|u| !u.is_empty()) {
            if u.starts_with('/') && Path::new(u).is_file() {
                return Some(PathBuf::from(u));
            }
            let hash = Sha256::digest(u.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
            let p = base.join("images-cache").join(hash);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn lutris_art(slug: &str, kind: Art) -> Option<PathBuf> {
    let h = home();
    let dirs: &[&str] = match kind {
        Art::Cover => &["coverart", "banners"],
        Art::Hero => &["banners", "coverart"],
        Art::Logo => return None,
    };
    let roots = [lutris_data_dirs(), vec![h.join(".cache/lutris")]].concat();
    for d in dirs {
        for r in &roots {
            for ext in ["jpg", "png"] {
                let p = r.join(d).join(format!("{slug}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn data_url(p: &Path) -> Option<String> {
    let b = fs::read(p).ok()?;
    // capas da Steam têm ~80 KB e banners ~400 KB; algo muito maior deixaria a tela pesada
    if b.len() > 3 * 1024 * 1024 {
        return None;
    }
    let mime = if b.starts_with(b"\x89PNG") {
        "image/png"
    } else if b.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        return None;
    };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&b)))
}

// ---------------------------------------------------------------- abrir

/// Endereço que a loja entende para abrir o jogo.
pub fn launch_uri(g: &Game) -> Option<String> {
    let (store, rest) = g.key.split_once(':')?;
    match store {
        "steam" if g.id.chars().all(|c| c.is_ascii_digit()) => Some(format!("steam://rungameid/{}", g.id)),
        "heroic" => {
            let (runner, app) = rest.split_once(':')?;
            (safe_id(app) && ["legendary", "gog", "nile", "sideload"].contains(&runner)).then(|| format!("heroic://launch?appName={app}&runner={runner}"))
        }
        "lutris" if g.id.chars().all(|c| c.is_ascii_digit()) => Some(format!("lutris:rungameid/{}", g.id)),
        _ => None,
    }
}

/// Abre `target` (URL ou pasta) com o programa padrão, sem esperar: o xdg-open entrega à loja e sai.
pub fn xdg_open(target: &str) -> Result<(), String> {
    let mut child = Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("não deu para rodar o xdg-open: {e}"))?;
    // recolhe o processo quando ele terminar, para não sobrar zumbi
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ---------------------------------------------------------------- jogo rodando agora

fn proc_cmdlines() -> impl Iterator<Item = (u32, Vec<String>)> {
    fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| {
        let pid: u32 = e.file_name().to_str()?.parse().ok()?;
        let raw = fs::read(e.path().join("cmdline")).ok()?;
        if raw.is_empty() {
            return None;
        }
        let args = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        Some((pid, args))
    })
}

/// appid de um processo lançado pela Steam: o "reaper" recebe `SteamLaunch AppId=<id>`.
fn steam_appid_of(args: &[String]) -> Option<&str> {
    if !args.iter().any(|a| a == "SteamLaunch") {
        return None;
    }
    args.iter().find_map(|a| a.strip_prefix("AppId=")).filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && *id != "0")
}

/// Nome do jogo da biblioteca que está rodando agora, se houver.
pub fn running_game(games: &[Game]) -> Option<String> {
    for (_, args) in proc_cmdlines() {
        if let Some(id) = steam_appid_of(&args) {
            return Some(games.iter().find(|g| g.is_steam() && g.id == id).map(|g| g.name.clone()).unwrap_or_else(|| format!("app {id} da Steam")));
        }
        for g in games.iter().filter(|g| !g.is_steam() && g.install_dir.len() > 4) {
            if args.iter().any(|a| a.starts_with(&g.install_dir)) {
                return Some(g.name.clone());
            }
        }
    }
    None
}

/// Programas que mudam o desempenho do jogo e que as opções de inicialização podem usar.
#[derive(Debug, Clone, Serialize)]
pub struct Tools {
    pub gamemode: bool,
    pub mangohud: bool,
    pub gamescope: bool,
}

pub fn tools() -> Tools {
    Tools { gamemode: in_path("gamemoderun"), mangohud: in_path("mangohud"), gamescope: in_path("gamescope") }
}

fn in_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest() {
        let t = r#""AppState"
{
	"appid"		"1174180"
	"name"		"Red Dead Redemption 2"
	"StateFlags"		"518"
	"installdir"		"Red Dead Redemption 2"
	"LastPlayed"		"1792279477"
	"SizeOnDisk"		"128281316135"
}"#;
        let m = parse_manifest(t).unwrap();
        assert_eq!(m.appid, "1174180");
        assert_eq!(m.name, "Red Dead Redemption 2");
        assert_eq!(m.state_flags & 4, 4);
        assert_eq!(m.state_flags & 2, 2); // atualização pendente
        assert_eq!(m.size, Some(128281316135));
        assert_eq!(m.last_played, Some(1792279477));
        assert!(parse_manifest("\"AppState\" { \"appid\" \"../x\" \"name\" \"a\" \"installdir\" \"b\" }").is_none());
    }

    #[test]
    fn tools_are_not_games() {
        assert!(is_steam_tool("1493710", "Proton Experimental"));
        assert!(is_steam_tool("4183110", "Steam Linux Runtime 4.0"));
        assert!(is_steam_tool("228980", "Steamworks Common Redistributables"));
        assert!(!is_steam_tool("2358720", "Black Myth: Wukong"));
    }

    #[test]
    fn library_folders_old_and_new() {
        let v = vdf::parse(r#""libraryfolders" { "contentstatsid" "123" "1" "/old/lib" "2" { "path" "/mnt/jogos/SteamLibrary" } }"#);
        assert_eq!(library_paths(&v), [PathBuf::from("/old/lib"), PathBuf::from("/mnt/jogos/SteamLibrary")]);
    }

    #[test]
    fn sizes_and_dates() {
        assert_eq!(parse_size(&serde_json::json!(1234)), Some(1234));
        assert_eq!(parse_size(&serde_json::json!("1.5 GB")), Some(1610612736));
        assert_eq!(parse_size(&serde_json::json!("512 MiB")), Some(536870912));
        assert_eq!(parse_size(&serde_json::json!("0")), None);
        assert_eq!(parse_iso("1970-01-02T00:00:00.000Z"), Some(86400));
        assert_eq!(parse_iso("2025-03-01T18:22:05.000Z"), Some(1740853325));
        assert_eq!(parse_iso("lixo"), None);
    }

    #[test]
    fn heroic_meta_is_found_anywhere() {
        let v = serde_json::json!({ "library": [ { "app_name": "Fortnite", "title": "Fortnite", "art_square": "https://x/y.jpg" } ], "outro": { "games": [ { "app_name": "123", "title": "Witcher" } ] } });
        let mut m = HashMap::new();
        collect_meta(&v, &mut m);
        assert_eq!(m["Fortnite"].art_square, "https://x/y.jpg");
        assert_eq!(m["123"].title, "Witcher");
    }

    #[test]
    fn uris_and_ids() {
        let mut g = Game {
            key: "steam:730".into(), store: "Steam".into(), id: "730".into(), name: "CS2".into(), install_dir: String::new(), size_bytes: None,
            last_played: None, playtime_min: None, platform: "Nativo".into(), source: String::new(), update_pending: false, launch_options: None,
        };
        assert_eq!(launch_uri(&g).as_deref(), Some("steam://rungameid/730"));
        g.key = "heroic:legendary:Fortnite".into();
        g.id = "Fortnite".into();
        assert_eq!(launch_uri(&g).as_deref(), Some("heroic://launch?appName=Fortnite&runner=legendary"));
        g.key = "heroic:legendary:a&b=c".into();
        assert_eq!(launch_uri(&g), None);
        assert!(!safe_slug("../etc"));
        assert_eq!(steam_appid_of(&["reaper".into(), "SteamLaunch".into(), "AppId=1174180".into(), "--".into()]), Some("1174180"));
        assert_eq!(steam_appid_of(&["reaper".into(), "AppId=1".into()]), None);
    }
}
