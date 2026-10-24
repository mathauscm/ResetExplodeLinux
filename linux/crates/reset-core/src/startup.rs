//! Inicialização: o que abre junto com o sistema (equivalente ao StartupManager.cs do Windows).
//!
//! - Apps ao entrar na sessão: atalhos .desktop em ~/.config/autostart (do usuário) e /etc/xdg/autostart
//!   (do sistema). Desativar um do sistema usa o padrão do XDG: um arquivo com o mesmo nome em
//!   ~/.config/autostart com `Hidden=true` (o do sistema fica intacto). Remover só vale para os do usuário,
//!   com cópia para "restaurar removidos", como no Windows.
//! - Serviços do usuário (`systemctl --user`): desativar não pede senha.
//! - Serviços do sistema: só com root (pkexec), e os essenciais ficam bloqueados.
//! - Tempo de boot (`systemd-analyze`, sem root) e o "impacto" de cada item: quanto atrasou o boot,
//!   memória e CPU que usa agora (lidos do cgroup de cada serviço ou app).

use crate::tweaks::Plan;
use crate::util::read_trim;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SYSTEMCTL: &str = "/usr/bin/systemctl";
/// Marca dos arquivos que o app cria em ~/.config/autostart para desativar um atalho do sistema.
const OVERRIDE_KEY: &str = "X-ResetExplode-Override";
/// Marca do app num atalho do usuário que ele desativou (para saber que pode desfazer).
const DISABLED_KEY: &str = "X-ResetExplode-Disabled";

/// Pastas usadas. Os testes apontam tudo para uma pasta temporária.
#[derive(Debug, Clone)]
pub struct Paths {
    pub user_autostart: PathBuf,
    /// Em ordem de prioridade (XDG_CONFIG_DIRS + /autostart).
    pub system_autostart: Vec<PathBuf>,
    /// ~/.config/reset-explode: registro do que o app desativou e cópias do que foi removido.
    pub data: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self, String> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or("pasta de configuração não encontrada")?;
        let dirs = std::env::var("XDG_CONFIG_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/etc/xdg".into());
        Ok(Paths {
            user_autostart: config.join("autostart"),
            system_autostart: dirs.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("autostart")).collect(),
            data: config.join("reset-explode"),
        })
    }

    fn state_file(&self) -> PathBuf {
        self.data.join("startup.json")
    }

    fn backup_dir(&self) -> PathBuf {
        self.data.join("startup-removidos")
    }
}

// ---------------------------------------------------------------- registro

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Removed {
    pub name: String,
    pub original: String,
    /// Nome do arquivo na pasta de cópias (é o que a interface manda de volta para restaurar).
    pub backup: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct DisabledUnit {
    /// "user" ou "system"
    scope: String,
    unit: String,
    also: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    disabled: Vec<DisabledUnit>,
    removed: Vec<Removed>,
}

fn load_state(p: &Paths) -> State {
    fs::read_to_string(p.state_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_state(p: &Paths, s: &State) -> Result<(), String> {
    fs::create_dir_all(&p.data).map_err(|e| e.to_string())?;
    fs::write(p.state_file(), serde_json::to_string_pretty(s).unwrap_or_default()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- itens

#[derive(Debug, Clone, Default, Serialize)]
pub struct Impact {
    /// "alto", "médio", "baixo" ou "" (sem dados)
    pub level: &'static str,
    /// Quanto o serviço levou para subir no boot (systemd-analyze blame).
    pub boot_ms: Option<u64>,
    /// Memória que usa agora.
    pub memory: Option<u64>,
    /// CPU usada desde que abriu.
    pub cpu_ms: Option<u64>,
    pub running: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Entry {
    /// "autostart:<arquivo>.desktop", "user:<unidade>" ou "system:<unidade>"
    pub id: String,
    /// "autostart", "user" ou "system"
    pub kind: &'static str,
    pub name: String,
    pub comment: String,
    /// Data URL do ícone (só dos atalhos .desktop), ou vazio.
    pub icon: String,
    pub command: String,
    pub source: String,
    pub enabled: bool,
    /// Falso quando este atalho não roda nesta sessão (é de outro desktop, ou o GNOME usa um serviço no lugar).
    pub applies: bool,
    pub note: String,
    /// O programa do atalho não existe mais.
    pub missing: bool,
    /// Por que não dá para mexer (vazio = pode).
    pub locked: String,
    pub can_remove: bool,
    /// Foi o app que desativou (o "desfazer" é reativar).
    pub by_us: bool,
    pub risk: &'static str,
    /// O que é, para os conhecidos.
    pub hint: String,
    pub impact: Impact,
    /// Unidades que vão junto (avahi-daemon.service leva o avahi-daemon.socket).
    pub also: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Startup {
    pub entries: Vec<Entry>,
    pub removed: Vec<Removed>,
    /// Desktop desta sessão (XDG_CURRENT_DESKTOP).
    pub desktop: String,
}

// ---------------------------------------------------------------- .desktop

/// Campos do grupo [Desktop Entry].
pub fn parse_entry(text: &str) -> HashMap<String, String> {
    let mut in_entry = false;
    let mut f = HashMap::new();
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if in_entry && !l.starts_with('#') {
            if let Some((k, v)) = l.split_once('=') {
                f.entry(k.trim().to_string()).or_insert_with(|| v.trim().to_string());
            }
        }
    }
    f
}

fn localized(f: &HashMap<String, String>, key: &str) -> Option<String> {
    [format!("{key}[pt_BR]"), format!("{key}[pt]"), key.to_string()].iter().find_map(|k| f.get(k)).cloned()
}

fn is_true(f: &HashMap<String, String>, key: &str) -> bool {
    f.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Muda chaves do grupo [Desktop Entry] mantendo o resto do arquivo: `Some` troca (ou acrescenta no fim do
/// grupo), `None` apaga a linha.
pub fn set_keys(text: &str, edits: &[(&str, Option<&str>)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_entry = false;
    let mut done: HashSet<&str> = HashSet::new();
    let mut group_end: Option<usize> = None;
    let mut saw_entry = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if in_entry {
                group_end = Some(out.len());
            }
            in_entry = t == "[Desktop Entry]";
            saw_entry |= in_entry;
            out.push(l.to_string());
            continue;
        }
        if in_entry {
            if let Some((k, _)) = t.split_once('=') {
                if let Some((key, val)) = edits.iter().find(|(e, _)| *e == k.trim()) {
                    if let Some(v) = val {
                        if done.insert(key) {
                            out.push(format!("{key}={v}"));
                        }
                    }
                    continue;
                }
            }
        }
        out.push(l.to_string());
    }
    if !saw_entry {
        out.insert(0, "[Desktop Entry]".into());
    }
    // linhas novas entram no fim do grupo (antes de linhas vazias que o separam do próximo)
    let mut at = group_end.unwrap_or(out.len());
    while at > 0 && out[at - 1].trim().is_empty() {
        at -= 1;
    }
    let new: Vec<String> = edits.iter().filter(|(k, v)| v.is_some() && !done.contains(k)).map(|(k, v)| format!("{k}={}", v.unwrap())).collect();
    for (i, l) in new.into_iter().enumerate() {
        out.insert(at + i, l);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Primeiro programa da linha Exec (pula `env VAR=...`), sem aspas.
pub fn exec_program(exec: &str) -> Option<String> {
    let mut words = shell_words(exec).into_iter();
    let mut w = words.next()?;
    if w == "env" || w == "/usr/bin/env" {
        w = words.find(|x| !x.contains('=') && !x.starts_with('-'))?;
    }
    Some(w)
}

fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn program_exists(prog: &str) -> bool {
    if prog.contains('/') {
        let p = if let Some(rest) = prog.strip_prefix("~/") { std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_default() } else { PathBuf::from(prog) };
        return p.exists();
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// O atalho roda neste desktop? (OnlyShowIn/NotShowIn, e o GNOME troca alguns por serviços do systemd.)
pub fn applies_here(f: &HashMap<String, String>, desktop: &str) -> (bool, String) {
    let mine: Vec<String> = desktop.split(':').filter(|d| !d.is_empty()).map(|d| d.to_lowercase()).collect();
    let list = |k: &str| f.get(k).map(|v| v.split(';').filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect::<Vec<_>>());
    if let Some(only) = list("OnlyShowIn") {
        if !mine.is_empty() && !only.iter().any(|d| mine.contains(d)) {
            return (false, format!("Só roda em {}.", f["OnlyShowIn"].trim_end_matches(';').replace(';', ", ")));
        }
    }
    if let Some(not) = list("NotShowIn") {
        if not.iter().any(|d| mine.contains(d)) {
            return (false, format!("Não roda no {}.", desktop.split(':').next().unwrap_or(desktop)));
        }
    }
    if mine.iter().any(|d| d == "gnome") && is_true(f, "X-GNOME-HiddenUnderSystemd") {
        return (false, "No GNOME isto é iniciado por um serviço do systemd, não por este atalho.".into());
    }
    (true, String::new())
}

/// Atalhos do sistema que fazem parte da sessão: desligar quebraria senhas, acessibilidade, teclado...
const ESSENTIAL_AUTOSTART: &[&str] = &[
    "gnome-keyring", "at-spi", "org.gnome.SettingsDaemon", "org.gnome.Shell", "gnome-shell", "polkit", "xdg-desktop-portal",
    "pipewire", "wireplumber", "ibus", "org.freedesktop.IBus", "spice-vdagent", "user-dirs-update-gtk", "xdg-user-dirs",
];

/// (início do nome do arquivo, o que é)
const AUTOSTART_HINTS: &[(&str, &str)] = &[
    ("localsearch", "Indexa seus arquivos para a busca do GNOME. Desligar deixa a busca por arquivos mais pobre."),
    ("tracker", "Indexa seus arquivos para a busca do GNOME. Desligar deixa a busca por arquivos mais pobre."),
    ("geoclue", "Agente de localização (usado por clima, fuso horário automático e mapas)."),
    ("limine-snapper-notify", "Avisa quando o PC iniciou a partir de um snapshot do snapper (restauração)."),
    ("limine-restore-notify", "Avisa quando o PC iniciou a partir de um snapshot do snapper (restauração)."),
    ("Toshy", "Toshy: atalhos de teclado no estilo do Mac."),
    ("org.gnome.Software", "Loja do GNOME procurando atualizações em segundo plano."),
    ("gnome-software", "Loja do GNOME procurando atualizações em segundo plano."),
    ("steam", "Abre a Steam ao entrar (minimizada, se configurado nela)."),
    ("discord", "Abre o Discord ao entrar."),
    ("solaar", "Configura mouses e teclados Logitech."),
    ("openrgb", "OpenRGB ao entrar."),
];

struct Desktop {
    file: String,
    fields: HashMap<String, String>,
}

fn read_dir_desktops(dir: &Path) -> Vec<Desktop> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Desktop> = rd
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            if !file.ends_with(".desktop") {
                return None;
            }
            let text = fs::read_to_string(e.path()).ok()?;
            Some(Desktop { file, fields: parse_entry(&text) })
        })
        .collect();
    v.sort_by(|a, b| a.file.cmp(&b.file));
    v
}

/// Atalhos de autostart, já mesclados (o do usuário vale sobre o do sistema com o mesmo nome).
pub fn read_autostart(p: &Paths, desktop: &str) -> Vec<Entry> {
    let user: HashMap<String, Desktop> = read_dir_desktops(&p.user_autostart).into_iter().map(|d| (d.file.clone(), d)).collect();
    let mut system: HashMap<String, Desktop> = HashMap::new();
    for dir in &p.system_autostart {
        for d in read_dir_desktops(dir) {
            system.entry(d.file.clone()).or_insert(d);
        }
    }
    let mut files: Vec<&String> = user.keys().chain(system.keys()).collect();
    files.sort();
    files.dedup();

    let mut out = Vec::new();
    for file in files {
        let u = user.get(file);
        let s = system.get(file);
        let override_only = u.is_some_and(|u| is_true(&u.fields, OVERRIDE_KEY));
        // nome, ícone e comando vêm do atalho "de verdade": o do sistema quando o do usuário é só a marca do app
        let main = match (u, s) {
            (Some(u), Some(s)) if override_only || !u.fields.contains_key("Exec") => s,
            (Some(u), _) => u,
            (None, Some(s)) => s,
            (None, None) => continue,
        };
        let eff = u.unwrap_or(main);
        if eff.fields.get("Type").is_some_and(|t| t != "Application") && !override_only {
            continue;
        }
        let f = &main.fields;
        let hidden = is_true(&eff.fields, "Hidden");
        let gnome_off = eff.fields.get("X-GNOME-Autostart-enabled").is_some_and(|v| v.eq_ignore_ascii_case("false"));
        let (applies, note) = applies_here(f, desktop);
        let exec = f.get("Exec").cloned().unwrap_or_default();
        let try_exec = f.get("TryExec").cloned();
        let missing = match try_exec.as_deref().or(exec_program(&exec).as_deref()) {
            Some(prog) => !program_exists(prog),
            None => false,
        };
        let stem = file.trim_end_matches(".desktop");
        let mut locked = String::new();
        if s.is_some() && ESSENTIAL_AUTOSTART.iter().any(|e| stem.starts_with(e)) {
            locked = "faz parte da sessão do sistema".into();
        }
        if stem == "reset-explode-lights" {
            locked = "ligue e desligue na tela Iluminação RGB".into();
        }
        let hint = AUTOSTART_HINTS.iter().find(|(k, _)| stem.to_lowercase().starts_with(&k.to_lowercase())).map(|(_, h)| h.to_string()).unwrap_or_default();
        out.push(Entry {
            id: format!("autostart:{file}"),
            kind: "autostart",
            name: localized(f, "Name").unwrap_or_else(|| stem.to_string()),
            comment: localized(f, "Comment").or_else(|| localized(f, "GenericName")).unwrap_or_default(),
            icon: f.get("Icon").cloned().unwrap_or_default(),
            command: exec,
            source: if s.is_some() { "Autostart do sistema (/etc/xdg/autostart)".into() } else { "Autostart do seu usuário (~/.config/autostart)".into() },
            enabled: !hidden && !gnome_off,
            applies,
            note,
            missing,
            locked,
            can_remove: s.is_none(),
            by_us: override_only || u.is_some_and(|u| is_true(&u.fields, DISABLED_KEY)),
            risk: if s.is_some() { "médio" } else { "baixo" },
            hint,
            impact: Impact::default(),
            also: Vec::new(),
        });
    }
    out
}

fn valid_desktop_file(file: &str) -> bool {
    file.ends_with(".desktop") && !file.starts_with('.') && !file.contains('/') && !file.contains('\0') && file.len() < 256
}

/// Liga ou desliga um atalho de autostart.
pub fn set_autostart(p: &Paths, file: &str, desktop: &str, enabled: bool) -> Result<String, String> {
    if !valid_desktop_file(file) {
        return Err("atalho inválido".into());
    }
    let entry = read_autostart(p, desktop).into_iter().find(|e| e.id == format!("autostart:{file}")).ok_or("atalho não encontrado")?;
    if !entry.locked.is_empty() {
        return Err(format!("{}: {}", entry.name, entry.locked));
    }
    let user_file = p.user_autostart.join(file);
    let user_text = fs::read_to_string(&user_file).ok();
    if enabled {
        let Some(text) = user_text else { return Ok(format!("{} já está ativado.", entry.name)) };
        let f = parse_entry(&text);
        let has_system = !entry.can_remove;
        if is_true(&f, OVERRIDE_KEY) && has_system {
            fs::remove_file(&user_file).map_err(|e| e.to_string())?;
        } else {
            let gnome = f.get("X-GNOME-Autostart-enabled").is_some_and(|v| v.eq_ignore_ascii_case("false")).then_some("true");
            let mut edits = vec![("Hidden", None), (DISABLED_KEY, None)];
            if gnome.is_some() {
                edits.push(("X-GNOME-Autostart-enabled", gnome));
            }
            fs::write(&user_file, set_keys(&text, &edits)).map_err(|e| e.to_string())?;
        }
        return Ok(format!("{} ativado. Vale no próximo login.", entry.name));
    }
    fs::create_dir_all(&p.user_autostart).map_err(|e| e.to_string())?;
    match user_text {
        Some(text) => fs::write(&user_file, set_keys(&text, &[("Hidden", Some("true")), (DISABLED_KEY, Some("true"))])),
        None => {
            // atalho do sistema: o arquivo do usuário com o mesmo nome e Hidden=true o esconde
            let name: String = entry.name.chars().filter(|c| !c.is_control()).collect();
            fs::write(&user_file, format!("[Desktop Entry]\nType=Application\nName={name}\nHidden=true\n{OVERRIDE_KEY}=true\n"))
        }
    }
    .map_err(|e| e.to_string())?;
    Ok(format!("{} desativado. Vale no próximo login.", entry.name))
}

/// Tira um atalho do usuário da inicialização, guardando uma cópia.
pub fn remove_autostart(p: &Paths, file: &str, desktop: &str) -> Result<String, String> {
    if !valid_desktop_file(file) {
        return Err("atalho inválido".into());
    }
    let entry = read_autostart(p, desktop).into_iter().find(|e| e.id == format!("autostart:{file}")).ok_or("atalho não encontrado")?;
    if !entry.can_remove || !entry.locked.is_empty() {
        return Err("só dá para remover atalhos do seu usuário; os do sistema dá para desativar.".into());
    }
    let dir = p.backup_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let backup = format!("{stamp}-{file}");
    let original = p.user_autostart.join(file);
    // rename dentro do mesmo disco; se falhar (pastas em discos diferentes), copia e apaga
    if fs::rename(&original, dir.join(&backup)).is_err() {
        fs::copy(&original, dir.join(&backup)).map_err(|e| e.to_string())?;
        fs::remove_file(&original).map_err(|e| e.to_string())?;
    }
    let mut st = load_state(p);
    st.removed.push(Removed { name: entry.name.clone(), original: original.display().to_string(), backup });
    save_state(p, &st)?;
    Ok(format!("{} removido da inicialização. Dá para desfazer em \"Removidos\".", entry.name))
}

/// Devolve um atalho removido.
pub fn restore_autostart(p: &Paths, backup: &str) -> Result<String, String> {
    let mut st = load_state(p);
    let i = st.removed.iter().position(|r| r.backup == backup).ok_or("item não encontrado")?;
    let r = st.removed[i].clone();
    if r.backup.contains('/') || r.backup.starts_with('.') {
        return Err("cópia inválida".into());
    }
    let file = r.backup.split_once('-').map(|(_, f)| f).unwrap_or(&r.backup);
    if !valid_desktop_file(file) {
        return Err("cópia inválida".into());
    }
    let target = p.user_autostart.join(file);
    if target.exists() {
        return Err(format!("já existe um {file} na inicialização."));
    }
    fs::create_dir_all(&p.user_autostart).map_err(|e| e.to_string())?;
    let src = p.backup_dir().join(&r.backup);
    if fs::rename(&src, &target).is_err() {
        fs::copy(&src, &target).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&src);
    }
    st.removed.remove(i);
    save_state(p, &st)?;
    Ok(format!("{} voltou para a inicialização.", r.name))
}

// ---------------------------------------------------------------- serviços (systemd)

/// Nome de unidade seguro para a linha de comando.
pub fn valid_unit(u: &str) -> bool {
    let types = [".service", ".socket", ".timer", ".path"];
    !u.is_empty()
        && u.len() < 200
        && !u.starts_with('-')
        && types.iter().any(|t| u.ends_with(t))
        && u.chars().all(|c| c.is_ascii_alphanumeric() || ":_.@-\\".contains(c))
}

/// `systemctl list-unit-files --no-legend --plain`: (unidade, estado).
pub fn parse_unit_files(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect()
}

/// `systemctl show -p ...` de várias unidades: um bloco por unidade, separados por linha vazia.
pub fn parse_show(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    for block in text.split("\n\n") {
        let f: HashMap<String, String> = block.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(id) = f.get("Id") {
            out.insert(id.clone(), f);
        }
    }
    out
}

/// "{ path=/usr/bin/x ; argv[]=/usr/bin/x --a ; ignore_errors=no ; ... }" -> "/usr/bin/x --a"
pub fn exec_start(v: &str) -> String {
    v.split_once("argv[]=").map(|(_, r)| r.split(" ;").next().unwrap_or(r).trim().to_string()).unwrap_or_default()
}

/// Bloqueados no sistema: (início do nome, motivo).
const ESSENTIAL_SYSTEM: &[(&str, &str)] = &[
    ("systemd-", "peça do systemd"),
    ("dbus", "comunicação entre programas do sistema"),
    ("getty@", "terminal de login"),
    ("serial-getty@", "terminal de login"),
    ("autovt@", "terminal de login"),
    ("user@", "sessão do usuário"),
    ("polkit", "permissões do sistema"),
    ("gdm", "tela de login"),
    ("sddm", "tela de login"),
    ("lightdm", "tela de login"),
    ("greetd", "tela de login"),
    ("ly.", "tela de login"),
    ("display-manager", "tela de login"),
    ("NetworkManager.service", "conexão de rede"),
    ("NetworkManager-dispatcher", "conexão de rede"),
    ("wpa_supplicant", "Wi-Fi"),
    ("iwd", "Wi-Fi"),
    ("udisks2", "discos e pendrives"),
    ("upower", "energia e bateria"),
    ("accounts-daemon", "contas de usuário"),
    ("rtkit-daemon", "prioridade do áudio"),
    ("power-profiles-daemon", "perfis de energia"),
    ("tuned", "perfis de energia"),
    ("nvidia-", "driver da placa de vídeo (suspender e retomar)"),
    ("snapper", "snapshots do sistema (restauração)"),
    ("limine", "boot e snapshots"),
    ("grub", "boot"),
    ("cachyos-", "ajuste do CachyOS"),
    ("ufw", "firewall: não desative por aqui"),
    ("firewalld", "firewall: não desative por aqui"),
    ("nftables", "firewall: não desative por aqui"),
    ("iptables", "firewall: não desative por aqui"),
    ("apparmor", "segurança: não desative por aqui"),
    ("lvm2", "discos"),
    ("cryptsetup", "discos criptografados"),
    ("dm-event", "discos"),
    ("btrfs", "sistema de arquivos"),
    ("zram", "memória (zram)"),
    ("fstrim", "controlado na tela Otimização"),
    ("ananicy", "controlado na tela Otimização"),
    ("scx", "escalonador (scx-manager)"),
    ("pci-latency", "ajuste do CachyOS"),
    ("plymouth", "tela de boot"),
    ("remote-fs", "pastas de rede"),
    ("auditd", "segurança: não desative por aqui"),
];

/// Do sistema que dá para desativar, com o que perde: (unidade, risco, explicação).
const SYSTEM_HINTS: &[(&str, &str, &str)] = &[
    ("NetworkManager-wait-online.service", "baixo", "Faz o boot esperar a internet conectar. Só faz falta se você monta pastas de rede no boot. Desativar costuma tirar alguns segundos do boot."),
    ("avahi-daemon.service", "baixo", "Encontra impressoras, Chromecast e outros aparelhos na rede local. Sem ele, a impressora em rede pode precisar ser adicionada pelo IP."),
    ("bluetooth.service", "baixo", "Bluetooth. Desativado, fones, controles e mouses Bluetooth param de funcionar."),
    ("sshd.service", "baixo", "Acesso a este PC pelo terminal, a partir de outro computador (SSH). Se você não usa, desativar é até mais seguro."),
    ("cups.service", "médio", "Impressão. Desativado, não dá para imprimir."),
    ("ModemManager.service", "baixo", "Modems 3G/4G. Sem utilidade em PC sem modem."),
    ("docker.service", "médio", "Docker. Desativado, os contêineres não sobem no boot."),
    ("libvirtd.service", "médio", "Máquinas virtuais (libvirt/virt-manager)."),
    ("smb.service", "médio", "Compartilhamento de pastas com Windows (Samba)."),
    ("nmb.service", "médio", "Nome do PC na rede Windows (Samba)."),
    ("teamviewerd.service", "baixo", "Acesso remoto do TeamViewer."),
    ("anydesk.service", "baixo", "Acesso remoto do AnyDesk."),
    ("cronie.service", "médio", "Tarefas agendadas (cron)."),
    ("reflector.timer", "baixo", "Atualiza a lista de espelhos do pacman de tempos em tempos."),
    ("paccache.timer", "baixo", "Limpa o cache do pacman toda semana."),
    ("lm_sensors.service", "baixo", "Carrega módulos de sensores no boot."),
];

/// Bloqueados no usuário: fazem parte da sessão.
const ESSENTIAL_USER: &[(&str, &str)] = &[
    ("pipewire", "áudio do sistema"),
    ("wireplumber", "áudio do sistema"),
    ("gnome-keyring", "senhas e chaves"),
    ("gcr-", "senhas e chaves"),
    ("p11-kit", "certificados"),
    ("dbus", "comunicação entre programas"),
    ("xdg-desktop-portal", "janelas de abrir arquivo e compartilhar tela"),
    ("xdg-document-portal", "janelas de abrir arquivo"),
    ("xdg-permission-store", "permissões dos apps"),
    ("at-spi", "acessibilidade"),
    ("org.gnome.", "sessão do GNOME"),
    ("gvfs", "pendrives e pastas de rede no gerenciador de arquivos"),
    ("xdg-user-dirs", "pastas Documentos, Downloads..."),
    ("localsearch", "busca de arquivos do GNOME"),
    ("tracker", "busca de arquivos do GNOME"),
    ("evolution-", "agenda e contas do GNOME"),
];

/// Por que a unidade não pode ser mexida (vazio = pode), risco e explicação.
pub fn classify(scope: &str, unit: &str) -> (String, &'static str, String) {
    let list = if scope == "system" { ESSENTIAL_SYSTEM } else { ESSENTIAL_USER };
    if let Some((_, why)) = list.iter().find(|(p, _)| unit.starts_with(p)) {
        return (why.to_string(), "alto", String::new());
    }
    if scope == "system" {
        if let Some((_, risk, hint)) = SYSTEM_HINTS.iter().find(|(u, _, _)| *u == unit) {
            return (String::new(), risk, hint.to_string());
        }
        return (String::new(), "médio", "Serviço que não é do núcleo do sistema. Só desative se souber o que ele faz.".into());
    }
    (String::new(), "baixo", String::new())
}

fn systemctl(user: bool, args: &[&str]) -> String {
    let mut c = Command::new(SYSTEMCTL);
    if user {
        c.arg("--user");
    }
    c.args(args).env("LC_ALL", "C").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Junta X.service com X.socket/X.path ligados: aparecem como um item e são desativados juntos.
pub fn group_companions(units: &[String]) -> Vec<(String, Vec<String>)> {
    let set: HashSet<&str> = units.iter().map(|s| s.as_str()).collect();
    let mut hidden = HashSet::new();
    let mut out = Vec::new();
    for u in units {
        if let Some(stem) = u.strip_suffix(".service") {
            let also: Vec<String> = [".socket", ".path"].iter().map(|s| format!("{stem}{s}")).filter(|c| set.contains(c.as_str())).collect();
            hidden.extend(also.clone());
            out.push((u.clone(), also));
        }
    }
    for u in units {
        if !u.ends_with(".service") && !hidden.contains(u) {
            out.push((u.clone(), Vec::new()));
        }
    }
    out
}

fn read_units(scope: &str, recorded: &[DisabledUnit]) -> Vec<Entry> {
    let user = scope == "user";
    let files = parse_unit_files(&systemctl(user, &["list-unit-files", "--state=enabled", "--no-legend", "--no-pager", "--plain"]));
    let mut units: Vec<String> = files.into_iter().map(|(u, _)| u).filter(|u| valid_unit(u) && !u.contains("@.")).collect();
    let ours: Vec<&DisabledUnit> = recorded.iter().filter(|d| d.scope == scope).collect();
    for d in &ours {
        if !units.contains(&d.unit) {
            units.push(d.unit.clone());
        }
    }
    units.sort();
    let grouped = group_companions(&units);
    if grouped.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["show", "--property=Id,Description,ActiveState,UnitFileState,FragmentPath,ExecStart", "--"];
    args.extend(grouped.iter().map(|(u, _)| u.as_str()));
    let show = parse_show(&systemctl(user, &args));

    grouped
        .into_iter()
        .filter_map(|(unit, also)| {
            let f = show.get(&unit).cloned().unwrap_or_default();
            let state = f.get("UnitFileState").cloned().unwrap_or_default();
            let enabled = state == "enabled" || state == "enabled-runtime";
            let by_us = ours.iter().any(|d| d.unit == unit) && !enabled;
            // da lista do app, mas alguém reativou ou a unidade sumiu: não mostra como "desativado pelo app"
            if !enabled && !by_us {
                return None;
            }
            let (locked, risk, hint) = classify(scope, &unit);
            let fragment = f.get("FragmentPath").cloned().unwrap_or_default();
            let command = f.get("ExecStart").map(|e| exec_start(e)).filter(|s| !s.is_empty()).unwrap_or_else(|| fragment.clone());
            let mine = fragment.contains("/.config/systemd/");
            Some(Entry {
                id: format!("{scope}:{unit}"),
                kind: if user { "user" } else { "system" },
                name: unit.trim_end_matches(".service").to_string(),
                comment: f.get("Description").cloned().unwrap_or_default(),
                command,
                source: if user {
                    if mine { "Serviço do usuário (criado por você ou por um app)".into() } else { "Serviço do usuário (systemd --user)".into() }
                } else {
                    "Serviço do sistema".into()
                },
                enabled,
                applies: true,
                locked,
                by_us,
                risk,
                hint,
                impact: Impact { running: f.get("ActiveState").is_some_and(|s| s == "active"), ..Default::default() },
                also,
                ..Default::default()
            })
        })
        .collect()
}

/// Resultado de ligar/desligar: pronto, ou um comando com root para a janela rodar.
#[derive(Debug, Clone)]
pub enum Outcome {
    Done(String),
    Job(Plan),
}

/// Liga ou desliga um item da lista (o `id` é conferido contra a lista lida agora, nunca usado direto).
pub fn set_enabled(p: &Paths, id: &str, enabled: bool) -> Result<Outcome, String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if let Some(file) = id.strip_prefix("autostart:") {
        return set_autostart(p, file, &desktop, enabled).map(Outcome::Done);
    }
    let (scope, unit) = id.split_once(':').ok_or("item inválido")?;
    if !(scope == "user" || scope == "system") || !valid_unit(unit) {
        return Err("item inválido".into());
    }
    let mut st = load_state(p);
    let entry = read_units(scope, &st.disabled).into_iter().find(|e| e.id == id).ok_or("serviço não encontrado")?;
    if !entry.locked.is_empty() {
        return Err(format!("{}: {}", entry.name, entry.locked));
    }
    if entry.enabled == enabled {
        return Ok(Outcome::Done(format!("{} já está {}.", entry.name, if enabled { "ativado" } else { "desativado" })));
    }
    let also = if enabled { st.disabled.iter().find(|d| d.scope == scope && d.unit == unit).map(|d| d.also.clone()).unwrap_or_default() } else { entry.also.clone() };
    if !also.iter().all(|u| valid_unit(u)) {
        return Err("item inválido".into());
    }
    let mut args: Vec<String> = vec![if enabled { "enable" } else { "disable" }.into(), "--".into(), unit.to_string()];
    args.extend(also.iter().cloned());

    // o registro vai antes; se a tarefa falhar, a próxima leitura vê a unidade ainda ligada e o descarta
    st.disabled.retain(|d| !(d.scope == scope && d.unit == unit));
    if !enabled {
        st.disabled.push(DisabledUnit { scope: scope.into(), unit: unit.into(), also });
    }
    save_state(p, &st)?;

    if scope == "user" {
        let mut full = vec!["--user".to_string()];
        full.extend(args);
        let out = Command::new(SYSTEMCTL).args(&full).env("LC_ALL", "C").output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        return Ok(Outcome::Done(format!("{} {}. Vale no próximo login.", entry.name, if enabled { "ativado" } else { "desativado" })));
    }
    Ok(Outcome::Job(Plan {
        root: true,
        program: SYSTEMCTL,
        args,
        title: format!("{} {}", if enabled { "Reativando" } else { "Desativando" }, entry.name),
    }))
}

// ---------------------------------------------------------------- impacto (cgroups)

/// Escapa como o systemd faz nos nomes de unidade ("google-chrome" -> "google\x2dchrome").
pub fn unit_escape(s: &str) -> String {
    let mut out = String::new();
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_alphanumeric() || b == b':' || b == b'_' || (b == b'.' && i > 0) {
            out.push(b as char);
        } else if b == b'/' {
            out.push('-');
        } else {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

/// O app de autostart vira "app-gnome-<id>-<pid>.scope" (GNOME) ou "app-<id>@autostart.service" (KDE).
pub fn scope_matches(cgroup_name: &str, desktop_id: &str) -> bool {
    let esc = unit_escape(desktop_id);
    cgroup_name.starts_with("app-") && (cgroup_name.contains(&format!("-{esc}-")) || cgroup_name.contains(&format!("-{esc}@")) || cgroup_name.starts_with(&format!("app-{esc}@")))
}

fn cgroup_stats(dir: &Path) -> (Option<u64>, Option<u64>) {
    let mem = read_trim(dir.join("memory.current")).and_then(|v| v.parse().ok());
    let cpu = fs::read_to_string(dir.join("cpu.stat"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse::<u64>().ok()))
        .map(|us| us / 1000);
    (mem, cpu)
}

/// Pastas do cgroup abaixo de `root` (até 3 níveis).
fn cgroup_dirs(root: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            out.push(e.path());
            cgroup_dirs(&e.path(), depth + 1, out);
        }
    }
}

/// Nível do impacto, a partir do que atrasou o boot, da memória e da CPU usadas.
pub fn impact_level(boot_ms: Option<u64>, memory: Option<u64>, cpu_ms: Option<u64>) -> &'static str {
    let (b, m, c) = (boot_ms.unwrap_or(0), memory.unwrap_or(0), cpu_ms.unwrap_or(0));
    if boot_ms.is_none() && memory.is_none() && cpu_ms.is_none() {
        ""
    } else if b >= 2000 || m >= 300 << 20 || c >= 60_000 {
        "alto"
    } else if b >= 500 || m >= 80 << 20 || c >= 5_000 {
        "médio"
    } else {
        "baixo"
    }
}

fn fill_impact(entries: &mut [Entry]) {
    let uid = unsafe { libc::getuid() };
    let user_root = PathBuf::from(format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service"));
    let mut user_dirs = Vec::new();
    cgroup_dirs(&user_root, 0, &mut user_dirs);
    let blame_sys = parse_blame(&analyze(false, &["blame", "--no-pager"]));
    let blame_user = parse_blame(&analyze(true, &["blame", "--no-pager"]));
    for e in entries.iter_mut() {
        let (dir, boot) = match e.kind {
            "autostart" => {
                let stem = e.id.trim_start_matches("autostart:").trim_end_matches(".desktop");
                (user_dirs.iter().find(|d| d.file_name().is_some_and(|n| scope_matches(&n.to_string_lossy(), stem))).cloned(), None)
            }
            "user" => {
                let unit = e.id.trim_start_matches("user:");
                (user_dirs.iter().find(|d| d.file_name().is_some_and(|n| n == unit)).cloned(), blame_user.iter().find(|(u, _)| u == unit).map(|(_, ms)| *ms))
            }
            _ => {
                let unit = e.id.trim_start_matches("system:");
                (Some(PathBuf::from("/sys/fs/cgroup/system.slice").join(unit)), blame_sys.iter().find(|(u, _)| u == unit).map(|(_, ms)| *ms))
            }
        };
        let (mem, cpu) = dir.as_deref().map(cgroup_stats).unwrap_or((None, None));
        let running = e.impact.running || mem.is_some();
        // processo parado não pesa agora: só conta o que atrasou o boot
        let (mem, cpu) = if running && e.enabled { (mem, cpu) } else { (None, None) };
        e.impact = Impact { level: impact_level(boot, mem, cpu), boot_ms: boot, memory: mem, cpu_ms: cpu, running };
    }
}

// ---------------------------------------------------------------- ícones

/// Nome do ícone -> arquivo. Cópia enxuta do índice de apps.rs (tema do usuário e herdados, hicolor, pixmaps).
struct IconIndex {
    files: HashMap<String, PathBuf>,
}

impl IconIndex {
    fn build(wanted: &HashSet<String>) -> Self {
        let mut files = HashMap::new();
        if wanted.is_empty() {
            return IconIndex { files };
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let roots = [home.join(".local/share/icons"), home.join(".icons"), PathBuf::from("/usr/share/icons")];
        let theme = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "icon-theme"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string())
            .unwrap_or_default();
        let mut themes = Vec::new();
        let mut next = vec![theme];
        while let Some(t) = next.pop() {
            if t.is_empty() || themes.contains(&t) || themes.len() > 6 {
                continue;
            }
            for r in &roots {
                if let Ok(text) = fs::read_to_string(r.join(&t).join("index.theme")) {
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
        let mut best: HashMap<String, (u32, PathBuf)> = HashMap::new();
        for (rank, t) in themes.iter().enumerate() {
            for r in &roots {
                walk(&r.join(t), 0, &mut |p| {
                    let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return };
                    if !wanted.contains(&stem) {
                        return;
                    }
                    let s = p.to_string_lossy();
                    let quality = match p.extension().and_then(|e| e.to_str()) {
                        Some("svg") => 300,
                        Some("png") => ["256", "128", "96", "64", "48", "32"].iter().position(|z| s.contains(&format!("/{z}x{z}")) || s.contains(&format!("/{z}/"))).map_or(10, |i| 200 - i as u32 * 10),
                        _ => return,
                    };
                    let score = (100 - rank as u32) * 1000 + quality;
                    if best.get(&stem).is_none_or(|(old, _)| score > *old) {
                        best.insert(stem, (score, p.to_path_buf()));
                    }
                });
            }
        }
        if let Ok(rd) = fs::read_dir("/usr/share/pixmaps") {
            for e in rd.flatten() {
                let p = e.path();
                if let (Some(stem), Some(ext)) = (p.file_stem(), p.extension()) {
                    let stem = stem.to_string_lossy().into_owned();
                    if (ext == "png" || ext == "svg") && wanted.contains(&stem) {
                        best.entry(stem).or_insert((0, p.clone()));
                    }
                }
            }
        }
        files.extend(best.into_iter().map(|(k, (_, p))| (k, p)));
        IconIndex { files }
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
        let Ok(meta) = fs::metadata(&p) else { continue };
        if meta.is_dir() {
            if e.file_name() != "symbolic" {
                walk(&p, depth + 1, f);
            }
        } else if meta.is_file() {
            f(&p);
        }
    }
}

/// Ícones extras de alguns serviços conhecidos (o resto fica com o desenho padrão da interface).
fn unit_icon_name(unit: &str) -> Option<&'static str> {
    let u = unit.to_lowercase();
    [("bluetooth", "bluetooth"), ("toshy", "toshy_app_icon_rainbow"), ("pipewire", "audio-card"), ("wireplumber", "audio-card"), ("docker", "docker")]
        .iter()
        .find(|(k, _)| u.starts_with(k))
        .map(|(_, i)| *i)
}

// ---------------------------------------------------------------- lista completa

/// Tudo que abre com o sistema, com ícones e impacto.
pub fn list(p: &Paths) -> Startup {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let mut st = load_state(p);
    let mut entries = read_autostart(p, &desktop);
    entries.extend(read_units("user", &st.disabled));
    entries.extend(read_units("system", &st.disabled));

    // registro do app: some o que não está mais desativado (reativado por fora, ou tarefa que falhou)
    let before = st.disabled.len();
    st.disabled.retain(|d| entries.iter().any(|e| e.id == format!("{}:{}", d.scope, d.unit) && !e.enabled));
    if st.disabled.len() != before {
        let _ = save_state(p, &st);
    }

    fill_impact(&mut entries);
    let wanted: HashSet<String> = entries
        .iter()
        .filter_map(|e| if e.kind == "autostart" { Some(e.icon.clone()) } else { unit_icon_name(&e.name).map(String::from) })
        .filter(|i| !i.is_empty() && !i.starts_with('/'))
        .collect();
    let icons = IconIndex::build(&wanted);
    for e in entries.iter_mut() {
        let name = if e.kind == "autostart" { e.icon.clone() } else { unit_icon_name(&e.name).unwrap_or("").to_string() };
        e.icon = if name.is_empty() { String::new() } else { icons.data_url(&name) };
    }
    Startup { entries, removed: st.removed, desktop }
}

// ---------------------------------------------------------------- tempo de boot

fn analyze(user: bool, args: &[&str]) -> String {
    let mut c = Command::new("systemd-analyze");
    if user {
        c.arg("--user");
    }
    c.args(args).env("LC_ALL", "C").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// "1min 2.345s", "877ms", "7.283s", "1h 2min" -> milissegundos.
pub fn parse_time(s: &str) -> Option<u64> {
    let mut total = 0.0;
    let mut any = false;
    for tok in s.split_whitespace() {
        let split = tok.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        let n: f64 = tok[..split].parse().ok()?;
        let mult = match &tok[split..] {
            "h" => 3_600_000.0,
            "min" => 60_000.0,
            "s" => 1000.0,
            "ms" => 1.0,
            "us" | "µs" => 0.001,
            _ => return None,
        };
        total += n * mult;
        any = true;
    }
    any.then(|| total.round() as u64)
}

/// `systemd-analyze blame`: (unidade, ms), na ordem da saída (mais lento primeiro).
pub fn parse_blame(text: &str) -> Vec<(String, u64)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (time, unit) = l.rsplit_once(' ')?;
            Some((unit.to_string(), parse_time(time)?))
        })
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct BootTimes {
    /// (fase, ms): firmware, loader, kernel, initrd, userspace
    pub parts: Vec<(String, u64)>,
    pub total_ms: u64,
    /// Até a tela de login (graphical.target), contando do kernel.
    pub target_ms: Option<u64>,
}

/// "Startup finished in 15.470s (firmware) + 2.558s (loader) + ... = 32.443s"
pub fn parse_analyze(text: &str) -> Option<BootTimes> {
    let line = text.lines().find(|l| l.starts_with("Startup finished in "))?;
    let body = line.trim_start_matches("Startup finished in ");
    // a sessão do usuário só tem uma fase: "Startup finished in 8.133s (userspace)"
    let (parts_s, total_s) = body.rsplit_once(" = ").unwrap_or((body, body.trim().split(" (").next().unwrap_or("")));
    let mut parts = Vec::new();
    for p in parts_s.split(" + ") {
        let (t, name) = p.trim().rsplit_once(" (")?;
        parts.push((name.trim_end_matches(')').to_string(), parse_time(t)?));
    }
    let target_ms = text.lines().find_map(|l| l.split_once(" reached after ").and_then(|(_, r)| parse_time(r.trim_end_matches('.').trim_end_matches(" in userspace"))));
    Some(BootTimes { parts, total_ms: parse_time(total_s.trim())?, target_ms })
}

#[derive(Debug, Clone, Serialize)]
pub struct Slow {
    pub unit: String,
    pub ms: u64,
    /// "system" ou "user"
    pub scope: &'static str,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Boot {
    pub times: Option<BootTimes>,
    /// Sessão do usuário (systemd --user), em ms.
    pub user_ms: Option<u64>,
    pub slow: Vec<Slow>,
    pub tips: Vec<String>,
}

/// Explicação para os lentos mais comuns.
pub fn slow_hint(unit: &str) -> String {
    match unit {
        "NetworkManager-wait-online.service" => "Só espera a internet conectar. Dá para desativar em Serviços do sistema.",
        "plymouth-quit-wait.service" => "Tela de carregamento esperando a tela de login: não é atraso de verdade.",
        "systemd-udev-settle.service" => "Espera todos os dispositivos aparecerem; algum pacote antigo ainda o usa.",
        "lvm2-monitor.service" => "Monitor de volumes LVM.",
        "ldconfig.service" | "man-db.service" | "updatedb.service" => "Manutenção que só roda de vez em quando (depois de atualizar).",
        u if u.starts_with("snapper") || u.starts_with("limine") => "Snapshots e entradas de boot (restauração do sistema).",
        u if u.starts_with("toshy") => "Serviço do Toshy.",
        _ => "",
    }
    .to_string()
}

/// Tempo de boot e o que mais pesou. Não precisa de root.
pub fn boot() -> Boot {
    let times = parse_analyze(&analyze(false, &[]));
    let user_ms = parse_analyze(&analyze(true, &[])).map(|t| t.total_ms);
    let only_services = |v: Vec<(String, u64)>| v.into_iter().filter(|(u, _)| u.ends_with(".service")).collect::<Vec<_>>();
    let sys = only_services(parse_blame(&analyze(false, &["blame", "--no-pager"])));
    let usr = only_services(parse_blame(&analyze(true, &["blame", "--no-pager"])));
    let mut slow: Vec<Slow> = sys.iter().take(8).map(|(u, ms)| Slow { hint: slow_hint(u), unit: u.clone(), ms: *ms, scope: "system" }).collect();
    slow.extend(usr.iter().take(5).map(|(u, ms)| Slow { hint: slow_hint(u), unit: u.clone(), ms: *ms, scope: "user" }));

    let mut tips = Vec::new();
    if let Some(t) = &times {
        let fw = t.parts.iter().find(|(n, _)| n == "firmware").map(|(_, ms)| *ms).unwrap_or(0);
        if fw >= 10_000 {
            tips.push(format!(
                "A placa-mãe leva {} s antes de o Linux começar. Ativar o Fast Boot na BIOS e desligar o que não usa (boot pela rede, controladora RAID) costuma cortar bastante.",
                format!("{:.1}", fw as f64 / 1000.0).replace('.', ",")
            ));
        }
    }
    if sys.iter().any(|(u, ms)| u == "NetworkManager-wait-online.service" && *ms >= 2000) {
        tips.push("O NetworkManager-wait-online faz o boot esperar a internet. Se você não monta pastas de rede no boot, pode desativar.".into());
    }
    Boot { times, user_ms, slow, tips }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> Paths {
        let d = std::env::temp_dir().join(format!("re-startup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let p = Paths { user_autostart: d.join("config/autostart"), system_autostart: vec![d.join("xdg/autostart")], data: d.join("config/reset-explode") };
        fs::create_dir_all(&p.user_autostart).unwrap();
        fs::create_dir_all(&p.system_autostart[0]).unwrap();
        p
    }

    fn cleanup(p: &Paths) {
        let _ = fs::remove_dir_all(p.user_autostart.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn edita_chaves_sem_estragar_o_resto() {
        let text = "[Desktop Entry]\nName=Toshy\nExec=toshy-tray\nHidden=false\n\n[Desktop Action x]\nName=Outra\nHidden=false\n";
        let out = set_keys(text, &[("Hidden", Some("true")), ("X-Novo", Some("1"))]);
        assert!(out.contains("[Desktop Entry]\nName=Toshy\nExec=toshy-tray\nHidden=true\nX-Novo=1\n\n[Desktop Action x]\nName=Outra\nHidden=false\n"));
        let back = set_keys(&out, &[("Hidden", None), ("X-Novo", None)]);
        assert_eq!(back, "[Desktop Entry]\nName=Toshy\nExec=toshy-tray\n\n[Desktop Action x]\nName=Outra\nHidden=false\n");
    }

    #[test]
    fn programa_da_linha_exec() {
        assert_eq!(exec_program("/home/u/.local/bin/toshy-tray").unwrap(), "/home/u/.local/bin/toshy-tray");
        assert_eq!(exec_program("\"/opt/My App/app\" --min %U").unwrap(), "/opt/My App/app");
        assert_eq!(exec_program("env FOO=1 BAR=2 steam -silent").unwrap(), "steam");
        assert!(exec_program("").is_none());
    }

    #[test]
    fn roda_neste_desktop() {
        let f = parse_entry("[Desktop Entry]\nNotShowIn=GNOME;\n");
        assert!(!applies_here(&f, "GNOME").0);
        let f = parse_entry("[Desktop Entry]\nOnlyShowIn=KDE;XFCE;\n");
        assert!(!applies_here(&f, "GNOME").0);
        assert!(applies_here(&f, "KDE").0);
        let f = parse_entry("[Desktop Entry]\nOnlyShowIn=GNOME;KDE;\nX-GNOME-HiddenUnderSystemd=true\n");
        assert!(!applies_here(&f, "ubuntu:GNOME").0);
        assert!(applies_here(&f, "KDE").0);
    }

    #[test]
    fn desativa_e_reativa_atalho_do_sistema() {
        let p = tmp("sys");
        fs::write(p.system_autostart[0].join("geo.desktop"), "[Desktop Entry]\nType=Application\nName=Geo\nName[pt_BR]=Localização\nExec=/bin/true\nIcon=mark-location\n").unwrap();
        let e = &read_autostart(&p, "GNOME")[0];
        assert_eq!((e.name.as_str(), e.enabled, e.can_remove, e.missing), ("Localização", true, false, false));

        set_autostart(&p, "geo.desktop", "GNOME", false).unwrap();
        let over = fs::read_to_string(p.user_autostart.join("geo.desktop")).unwrap();
        assert!(over.contains("Hidden=true") && over.contains(OVERRIDE_KEY));
        let e = &read_autostart(&p, "GNOME")[0];
        // o nome e o ícone continuam vindo do atalho do sistema
        assert_eq!((e.name.as_str(), e.icon.as_str(), e.enabled, e.by_us), ("Localização", "mark-location", false, true));
        // o do sistema não foi tocado
        assert!(!fs::read_to_string(p.system_autostart[0].join("geo.desktop")).unwrap().contains("Hidden"));

        set_autostart(&p, "geo.desktop", "GNOME", true).unwrap();
        assert!(!p.user_autostart.join("geo.desktop").exists());
        assert!(read_autostart(&p, "GNOME")[0].enabled);
        assert!(remove_autostart(&p, "geo.desktop", "GNOME").is_err());
        cleanup(&p);
    }

    #[test]
    fn desativa_remove_e_restaura_atalho_do_usuario() {
        let p = tmp("user");
        fs::write(p.user_autostart.join("app.desktop"), "[Desktop Entry]\nType=Application\nName=App\nExec=/nao/existe/app\nX-GNOME-Autostart-enabled=false\n").unwrap();
        let e = &read_autostart(&p, "GNOME")[0];
        assert!(!e.enabled && e.missing && e.can_remove && !e.by_us);

        // reativar um que o GNOME desligou
        set_autostart(&p, "app.desktop", "GNOME", true).unwrap();
        assert!(fs::read_to_string(p.user_autostart.join("app.desktop")).unwrap().contains("X-GNOME-Autostart-enabled=true"));
        set_autostart(&p, "app.desktop", "GNOME", false).unwrap();
        let e = &read_autostart(&p, "GNOME")[0];
        assert!(!e.enabled && e.by_us);
        set_autostart(&p, "app.desktop", "GNOME", true).unwrap();
        let text = fs::read_to_string(p.user_autostart.join("app.desktop")).unwrap();
        assert!(!text.contains("Hidden") && !text.contains(DISABLED_KEY) && text.contains("Exec=/nao/existe/app"));

        remove_autostart(&p, "app.desktop", "GNOME").unwrap();
        assert!(read_autostart(&p, "GNOME").is_empty());
        let st = load_state(&p);
        assert_eq!(st.removed.len(), 1);
        restore_autostart(&p, &st.removed[0].backup).unwrap();
        assert_eq!(read_autostart(&p, "GNOME").len(), 1);
        assert!(load_state(&p).removed.is_empty());

        assert!(set_autostart(&p, "../../etc/passwd", "GNOME", false).is_err());
        assert!(remove_autostart(&p, "nada.desktop", "GNOME").is_err());
        cleanup(&p);
    }

    #[test]
    fn essenciais_ficam_bloqueados() {
        let p = tmp("ess");
        fs::write(p.system_autostart[0].join("gnome-keyring-secrets.desktop"), "[Desktop Entry]\nType=Application\nName=Chaveiro\nExec=/bin/true\n").unwrap();
        assert!(!read_autostart(&p, "GNOME")[0].locked.is_empty());
        assert!(set_autostart(&p, "gnome-keyring-secrets.desktop", "GNOME", false).is_err());
        assert!(!p.user_autostart.join("gnome-keyring-secrets.desktop").exists());
        cleanup(&p);

        assert!(!classify("system", "systemd-resolved.service").0.is_empty());
        assert!(!classify("system", "gdm.service").0.is_empty());
        assert!(!classify("system", "NetworkManager.service").0.is_empty());
        assert!(classify("system", "NetworkManager-wait-online.service").0.is_empty());
        assert_eq!(classify("system", "sshd.service").1, "baixo");
        assert_eq!(classify("system", "proton.VPN.service").1, "médio");
        assert!(!classify("user", "pipewire.socket").0.is_empty());
        assert!(classify("user", "toshy-config.service").0.is_empty());
    }

    #[test]
    fn nomes_de_unidade() {
        assert!(valid_unit("NetworkManager-wait-online.service"));
        assert!(valid_unit("proton.VPN.service"));
        assert!(valid_unit("app-gnome-x\\x2dy.service"));
        assert!(!valid_unit("--root=/ x.service"));
        assert!(!valid_unit("a;reboot.service"));
        assert!(!valid_unit("graphical.target"));
        assert!(!valid_unit("-x.service"));
    }

    #[test]
    fn saidas_do_systemctl() {
        let f = parse_unit_files("avahi-daemon.service enabled disabled\navahi-daemon.socket enabled disabled\nfstrim.timer enabled disabled\n");
        assert_eq!(f.len(), 3);
        let units: Vec<String> = f.into_iter().map(|(u, _)| u).collect();
        let g = group_companions(&units);
        assert_eq!(g[0], ("avahi-daemon.service".to_string(), vec!["avahi-daemon.socket".to_string()]));
        assert_eq!(g[1].0, "fstrim.timer");
        assert_eq!(g.len(), 2);

        let s = parse_show("Id=a.service\nDescription=Um\nUnitFileState=enabled\n\nId=b.socket\nDescription=Dois\nUnitFileState=disabled\n");
        assert_eq!(s["b.socket"]["UnitFileState"], "disabled");
        assert_eq!(exec_start("{ path=/usr/bin/sshd ; argv[]=/usr/bin/sshd -D ; ignore_errors=no ; start_time=[n/a] }"), "/usr/bin/sshd -D");
    }

    #[test]
    fn escapes_e_cgroups() {
        assert_eq!(unit_escape("google-chrome"), "google\\x2dchrome");
        assert_eq!(unit_escape("Toshy_Tray"), "Toshy_Tray");
        assert!(scope_matches("app-gnome-google\\x2dchrome-8138.scope", "google-chrome"));
        assert!(scope_matches("app-gnome-Toshy_Tray-3698.scope", "Toshy_Tray"));
        assert!(scope_matches("app-org.kde.x@autostart.service", "org.kde.x"));
        assert!(!scope_matches("app-gnome-Toshy_Tray_Extra-1.scope", "Toshy_Tray"));
        assert_eq!(impact_level(Some(5036), None, None), "alto");
        assert_eq!(impact_level(None, Some(37 << 20), Some(8444)), "médio");
        assert_eq!(impact_level(Some(20), Some(5 << 20), Some(100)), "baixo");
        assert_eq!(impact_level(None, None, None), "");
    }

    #[test]
    fn tempos_do_systemd_analyze() {
        assert_eq!(parse_time("7.283s"), Some(7283));
        assert_eq!(parse_time("877ms"), Some(877));
        assert_eq!(parse_time("1min 2.5s"), Some(62500));
        assert_eq!(parse_time("abc"), None);
        let b = parse_blame("7.283s NetworkManager-wait-online.service\n 877ms toshy-config.service\n1min 3.100s algo-lento.service\n");
        assert_eq!(b[0], ("NetworkManager-wait-online.service".into(), 7283));
        assert_eq!(b[2], ("algo-lento.service".into(), 63100));
        let t = parse_analyze(
            "Startup finished in 15.470s (firmware) + 2.558s (loader) + 2.295s (kernel) + 2.467s (initrd) + 9.651s (userspace) = 32.443s \ngraphical.target reached after 8.526s in userspace.\n",
        )
        .unwrap();
        assert_eq!(t.total_ms, 32443);
        assert_eq!(t.parts[0], ("firmware".into(), 15470));
        assert_eq!(t.parts.len(), 5);
        assert_eq!(t.target_ms, Some(8526));
        let u = parse_analyze("Startup finished in 8.133s (userspace) \ndefault.target reached after 3.082s in userspace.\n").unwrap();
        assert_eq!(u.total_ms, 8133);
        assert!(parse_analyze("Bootup is not yet finished.").is_none());
    }
}
