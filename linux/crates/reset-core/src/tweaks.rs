//! Otimização: os equivalentes Linux dos "Ajustes do Windows para jogos" (WindowsTweaks.cs) e das limpezas.
//! Cada ajuste lê o estado atual, diz se foi o app que mudou (para poder desfazer) e reconhece o que o CachyOS
//! já deixa ajustado (pacote `cachyos-settings`, kernel próprio), para não mexer no que já está bom.
//!
//! Nada aqui roda com root: as ações com root viram um `Plan` (programa fixo + argumentos montados a partir de
//! valores validados) que a janela executa com `pkexec` (app/src-tauri/src/jobs.rs).
//! Os parâmetros do kernel (sysctl) ficam persistidos em /etc/sysctl.d/99-reset-explode.conf, com o valor de
//! antes em comentário, para o "desfazer" voltar exatamente ao que era.

use crate::packages::{self, PkgAction};
use crate::util::read_trim;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Arquivo de sysctl do app (o 99 vem por último e vale sobre os outros).
pub const SYSCTL_FILE: &str = "/etc/sysctl.d/99-reset-explode.conf";
const SYSCTL_DIRS: &[&str] = &["/etc/sysctl.d", "/run/sysctl.d", "/usr/local/lib/sysctl.d", "/usr/lib/sysctl.d"];

/// Um comando para a janela rodar (com root via pkexec se `root`). Programa sempre com caminho absoluto.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub root: bool,
    pub program: &'static str,
    pub args: Vec<String>,
    /// Título do painel de tarefa ("Aplicando: ...").
    pub title: String,
}

/// "applied" = o app aplicou (dá para desfazer); "ok" = já estava bom sem o app; "cachyos" = já ajustado pelo
/// CachyOS; "off" = não aplicado; "na" = não se aplica a este PC.
#[derive(Debug, Clone, Serialize)]
pub struct Tweak {
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    /// O que faz, em linguagem simples.
    pub what: &'static str,
    /// O que muda na prática (e o preço).
    pub effect: &'static str,
    /// "baixo", "médio" ou "alto"
    pub risk: &'static str,
    pub state: &'static str,
    /// Estado atual lido do sistema ("vm.swappiness = 150").
    pub current: String,
    /// O que o ajuste coloca.
    pub target: String,
    /// Detalhe do estado (de onde vem o valor, por que não dá para aplicar...).
    pub note: String,
    pub root: bool,
    pub can_apply: bool,
    pub can_undo: bool,
    pub apply_label: &'static str,
    pub undo_label: &'static str,
    /// Só vale depois de reiniciar.
    pub restart: bool,
}

// ---------------------------------------------------------------- perfil do sistema

#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub distro: String,
    pub kernel: String,
    pub cachyos: bool,
    /// Versão do pacote cachyos-settings (vazio se não instalado).
    pub cachyos_settings: String,
    pub cachyos_kernel: bool,
}

pub fn profile() -> Profile {
    let os = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |k: &str| os.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim_matches('"').to_string());
    let distro = field("PRETTY_NAME=").or_else(|| field("NAME=")).unwrap_or_else(|| "Linux".into());
    let id = field("ID=").unwrap_or_default();
    let kernel = read_trim("/proc/sys/kernel/osrelease").unwrap_or_default();
    let settings = pkg_version("cachyos-settings").unwrap_or_default();
    Profile {
        cachyos: id == "cachyos" || !settings.is_empty(),
        cachyos_kernel: kernel.contains("cachyos"),
        cachyos_settings: settings,
        distro,
        kernel,
    }
}

fn pkg_version(name: &str) -> Option<String> {
    let out = Command::new("pacman").args(["-Q", name]).env("LC_ALL", "C").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1).map(String::from)
}

// ---------------------------------------------------------------- registro do que o app mudou

/// O que o app mudou fora do sysctl e o valor de antes ("cpu" -> "balanced"). Fica em
/// ~/.config/reset-explode/tweaks.json. Se o sistema não está mais no valor do ajuste (alguém mudou depois,
/// ou a tarefa falhou), o registro é descartado na próxima leitura.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Record {
    before: BTreeMap<String, String>,
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|b| b.join("reset-explode"))
}

fn record_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("tweaks.json"))
}

fn load_record(path: Option<&Path>) -> Record {
    path.and_then(|p| fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_record(path: Option<&Path>, r: &Record) {
    let Some(p) = path else { return };
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = fs::write(p, serde_json::to_string_pretty(r).unwrap_or_default());
}

// ---------------------------------------------------------------- sysctl

struct SysTweak {
    id: &'static str,
    group: &'static str,
    title: &'static str,
    what: &'static str,
    effect: &'static str,
    risk: &'static str,
    /// Chaves e valores que o ajuste coloca.
    set: &'static [(&'static str, u64)],
    /// Chaves cujo valor de antes também é guardado (para desfazer): as de `set` e as que elas zeram.
    keep: &'static [&'static str],
}

const SYS_TWEAKS: &[SysTweak] = &[
    SysTweak {
        id: "swappiness",
        group: "Memória",
        title: "Usar menos o swap",
        what: "Faz o sistema preferir manter os programas na memória RAM em vez de mandar partes deles para o swap no disco.",
        effect: "Menos engasgos quando a RAM está quase cheia. Com zram (swap comprimido dentro da própria RAM) é o contrário: o valor alto é o certo.",
        risk: "baixo",
        set: &[("vm.swappiness", 10)],
        keep: &["vm.swappiness"],
    },
    SysTweak {
        id: "vfs_cache",
        group: "Memória",
        title: "Guardar mais pastas e arquivos em cache",
        what: "O sistema demora mais para descartar da memória a lista de pastas e arquivos que já leu.",
        effect: "Abrir pastas, jogos e programas de novo fica mais rápido. Usa um pouco mais de RAM, que é devolvida se faltar.",
        risk: "baixo",
        set: &[("vm.vfs_cache_pressure", 50)],
        keep: &["vm.vfs_cache_pressure"],
    },
    SysTweak {
        id: "dirty",
        group: "Disco",
        title: "Gravar no disco aos poucos",
        what: "Limita quanto dado fica na memória esperando para ser gravado (256 MB, começando a gravar com 64 MB).",
        effect: "Evita travadas longas quando um jogo, download ou cópia grande grava muito de uma vez.",
        risk: "baixo",
        set: &[("vm.dirty_bytes", 268_435_456), ("vm.dirty_background_bytes", 67_108_864)],
        // gravar *_bytes zera *_ratio (e vice-versa): guarda os quatro para voltar ao modo de antes
        keep: &["vm.dirty_bytes", "vm.dirty_background_bytes", "vm.dirty_ratio", "vm.dirty_background_ratio"],
    },
    SysTweak {
        id: "max_map_count",
        group: "Jogos",
        title: "Mais mapas de memória por programa",
        what: "Aumenta o limite de áreas de memória que um programa pode abrir (o padrão antigo era 65530).",
        effect: "Jogos novos (CS2, Hogwarts Legacy, Star Citizen e vários pelo Proton) param de fechar sozinhos com falta de memória.",
        risk: "baixo",
        set: &[("vm.max_map_count", 1_048_576)],
        keep: &["vm.max_map_count"],
    },
    SysTweak {
        id: "nmi_watchdog",
        group: "Kernel",
        title: "Desligar o vigia de travamentos (NMI watchdog)",
        what: "Um temporizador que interrompe a CPU de tempos em tempos para ver se o kernel travou.",
        effect: "Em PC de casa não faz falta: desligado, sobra um pouco de CPU e o consumo em repouso cai.",
        risk: "baixo",
        set: &[("kernel.nmi_watchdog", 0)],
        keep: &["kernel.nmi_watchdog"],
    },
    SysTweak {
        id: "split_lock",
        group: "Jogos",
        title: "Sem freio para jogos com \"split lock\"",
        what: "O kernel desacelera de propósito programas que fazem um tipo de acesso à memória chamado split lock, para não atrapalharem os outros.",
        effect: "Alguns jogos (God of War, jogos antigos pelo Proton) fazem isso o tempo todo e perdem muito FPS com o freio ligado.",
        risk: "baixo",
        set: &[("kernel.split_lock_mitigate", 0)],
        keep: &["kernel.split_lock_mitigate"],
    },
];

/// Toda chave que o app pode escrever (o que vier fora disto do arquivo é ignorado).
fn allowed_key(k: &str) -> bool {
    SYS_TWEAKS.iter().any(|t| t.keep.contains(&k))
}

/// O que está no 99-reset-explode.conf: valores aplicados e os de antes ("# antes: chave = valor").
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SysctlConf {
    pub values: BTreeMap<String, u64>,
    pub before: BTreeMap<String, u64>,
}

fn kv_line(l: &str) -> Option<(String, u64)> {
    let (k, v) = l.split_once('=')?;
    let k = k.trim();
    let v = v.trim().parse().ok()?;
    allowed_key(k).then(|| (k.to_string(), v))
}

pub fn parse_conf(text: &str) -> SysctlConf {
    let mut c = SysctlConf::default();
    for l in text.lines() {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix("# antes:") {
            if let Some((k, v)) = kv_line(rest) {
                c.before.insert(k, v);
            }
        } else if !l.starts_with('#') && !l.starts_with(';') {
            if let Some((k, v)) = kv_line(l) {
                c.values.insert(k, v);
            }
        }
    }
    c
}

pub fn render_conf(c: &SysctlConf) -> String {
    let mut s = String::from(
        "# Criado pelo RESET EXPLODE (Otimização). Desfaça pelo app ou apague este arquivo e reinicie.\n\
         # As linhas \"antes\" guardam o valor que o sistema tinha, para o desfazer.\n",
    );
    for (k, v) in &c.before {
        s.push_str(&format!("# antes: {k} = {v}\n"));
    }
    for (k, v) in &c.values {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

fn proc_path(key: &str) -> PathBuf {
    PathBuf::from("/proc/sys").join(key.replace('.', "/"))
}

fn read_sysctl(key: &str) -> Option<u64> {
    read_trim(proc_path(key))?.parse().ok()
}

/// Arquivo que define cada chave no boot, na ordem do systemd-sysctl: por nome de arquivo, o último vence,
/// e o mesmo nome em /etc esconde o de /usr/lib. `files` = (diretório, nome, conteúdo), na ordem de SYSCTL_DIRS.
pub fn sysctl_sources(files: &[(String, String, String)]) -> HashMap<String, (String, String)> {
    let mut by_name: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    for (dir, name, text) in files {
        by_name.entry(name.as_str()).or_insert((dir.as_str(), text.as_str()));
    }
    let mut out = HashMap::new();
    for (name, (dir, text)) in by_name {
        for l in text.lines() {
            let l = l.trim();
            if l.starts_with('#') || l.starts_with(';') {
                continue;
            }
            if let Some((k, v)) = l.split_once('=') {
                let k = k.trim().trim_start_matches('-').replace('/', ".");
                out.insert(k, (format!("{dir}/{name}"), v.trim().to_string()));
            }
        }
    }
    out
}

fn read_sysctl_files() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for d in SYSCTL_DIRS {
        let Ok(rd) = fs::read_dir(d) else { continue };
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".conf")).collect();
        names.sort();
        for n in names {
            let text = fs::read_to_string(Path::new(d).join(&n)).unwrap_or_default();
            out.push((d.to_string(), n, text));
        }
    }
    out
}

/// Script (sh) que grava o arquivo do app e aplica os valores na hora. Só entra chave da lista do app e número.
pub fn sysctl_script(conf: &SysctlConf, now: &[(String, u64)]) -> Result<String, String> {
    if let Some((k, _)) = conf.values.iter().chain(conf.before.iter()).chain(now.iter().map(|(k, v)| (k, v))).find(|(k, _)| !allowed_key(k)) {
        return Err(format!("chave não permitida: {k}"));
    }
    let mut s = String::from("set -e\n");
    if conf.values.is_empty() {
        s.push_str(&format!("rm -f {SYSCTL_FILE}\n"));
    } else {
        s.push_str(&format!("mkdir -p /etc/sysctl.d\ncat > {SYSCTL_FILE} <<'RESET_EOF'\n{}RESET_EOF\n", render_conf(conf)));
    }
    for (k, v) in now {
        s.push_str(&format!("sysctl -w {k}={v}\n"));
    }
    Ok(s)
}

/// Valores a gravar na hora para voltar ao de antes. Nos pares *_bytes/*_ratio, o que estava em 0 era o modo
/// desligado: gravar o outro já o zera (e gravar 0 em *_bytes daria erro).
fn undo_values(t: &SysTweak, before: &BTreeMap<String, u64>) -> Vec<(String, u64)> {
    t.keep
        .iter()
        .filter_map(|k| before.get(*k).map(|v| (k.to_string(), *v)))
        .filter(|(k, v)| !(*v == 0 && (k.ends_with("_bytes") || k.ends_with("_ratio"))))
        .collect()
}

/// Situação do sistema que decide o estado dos ajustes de sysctl.
pub struct SysEnv {
    pub current: HashMap<String, u64>,
    pub conf: SysctlConf,
    /// chave -> (arquivo que a define no boot, valor)
    pub sources: HashMap<String, (String, String)>,
    pub zram: bool,
    /// Regra do cachyos-settings que põe o swappiness em 150 quando o zram liga.
    pub cachyos_zram_rule: bool,
    pub cachyos_kernel: bool,
}

fn sys_state(t: &SysTweak, env: &SysEnv) -> Tweak {
    let fmt = |vals: Vec<(String, Option<u64>)>| {
        vals.iter().map(|(k, v)| format!("{k} = {}", v.map(|v| v.to_string()).unwrap_or("—".into()))).collect::<Vec<_>>().join(" · ")
    };
    let current = fmt(t.set.iter().map(|(k, _)| (k.to_string(), env.current.get(*k).copied())).collect());
    let target = fmt(t.set.iter().map(|(k, v)| (k.to_string(), Some(*v))).collect());
    let mut out = Tweak {
        id: t.id,
        group: t.group,
        title: t.title,
        what: t.what,
        effect: t.effect,
        risk: t.risk,
        state: "off",
        current,
        target,
        note: String::new(),
        root: true,
        can_apply: false,
        can_undo: false,
        apply_label: "Aplicar",
        undo_label: "Desfazer",
        restart: false,
    };
    if t.set.iter().any(|(k, _)| !env.current.contains_key(*k)) {
        out.state = "na";
        out.note = "Este kernel não tem essa opção.".into();
        return out;
    }
    let ours = t.set.iter().all(|(k, v)| env.conf.values.get(*k) == Some(v));
    let at_target = t.set.iter().all(|(k, v)| env.current.get(*k) == Some(v));
    let source = t.set.iter().filter_map(|(k, _)| env.sources.get(*k)).map(|(f, _)| f.clone()).find(|f| !f.ends_with("99-reset-explode.conf"));
    let from_cachyos = source.as_deref().is_some_and(|f| f.contains("cachyos"));

    if t.id == "swappiness" && env.zram {
        let v = env.current.get("vm.swappiness").copied().unwrap_or(0);
        out.state = if env.cachyos_zram_rule { "cachyos" } else { "ok" };
        out.target = "vm.swappiness alto (100 a 180) com zram".into();
        out.note = if env.cachyos_zram_rule {
            format!("O swap aqui é zram (comprimido na RAM). O CachyOS põe {v} de propósito: com zram, valor alto é mais rápido.")
        } else {
            format!("O swap aqui é zram (comprimido na RAM): {v} é adequado; valores baixos fariam o sistema descartar cache de disco.")
        };
        out.can_undo = ours;
        return out;
    }
    if ours {
        out.state = "applied";
        out.can_undo = true;
        out.note = if at_target { format!("Aplicado pelo app em {SYSCTL_FILE}.") } else { "Gravado pelo app, mas outro ajuste mudou o valor agora; vale depois de reiniciar.".into() };
        return out;
    }
    if at_target {
        if from_cachyos || (env.cachyos_kernel && source.is_none()) {
            out.state = "cachyos";
            out.note = match &source {
                Some(f) => format!("Já ajustado pelo CachyOS ({f})."),
                None => "Já vem assim no kernel do CachyOS.".into(),
            };
        } else {
            out.state = "ok";
            out.note = match &source {
                Some(f) => format!("Já está assim ({f})."),
                None => "Já está assim por padrão.".into(),
            };
        }
        return out;
    }
    out.can_apply = true;
    if let Some(f) = source {
        out.note = format!("Hoje vem de {f}. O arquivo do app vale por cima.");
    }
    out
}

fn sys_env(prof: &Profile) -> SysEnv {
    let mut current = HashMap::new();
    for t in SYS_TWEAKS {
        for k in t.keep {
            if let Some(v) = read_sysctl(k) {
                current.insert(k.to_string(), v);
            }
        }
    }
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    let rule = fs::read_to_string("/usr/lib/udev/rules.d/30-zram.rules").unwrap_or_default();
    SysEnv {
        current,
        conf: parse_conf(&fs::read_to_string(SYSCTL_FILE).unwrap_or_default()),
        sources: sysctl_sources(&read_sysctl_files()),
        zram: swaps.lines().skip(1).any(|l| l.starts_with("/dev/zram")),
        cachyos_zram_rule: !prof.cachyos_settings.is_empty() && rule.contains("vm.swappiness"),
        cachyos_kernel: prof.cachyos_kernel,
    }
}

/// Plano de aplicar/desfazer de um ajuste de sysctl, a partir do arquivo atual e dos valores de agora.
pub fn sysctl_plan(id: &str, apply: bool, env: &SysEnv) -> Result<Plan, String> {
    let t = SYS_TWEAKS.iter().find(|t| t.id == id).ok_or("ajuste desconhecido")?;
    let mut conf = env.conf.clone();
    let now: Vec<(String, u64)>;
    if apply {
        for k in t.keep {
            // o "antes" é o valor do sistema sem o app: se já havia um do app, mantém o antigo
            if !conf.before.contains_key(*k) {
                if let Some(v) = env.current.get(*k) {
                    conf.before.insert(k.to_string(), *v);
                }
            }
        }
        for (k, v) in t.set {
            conf.values.insert(k.to_string(), *v);
        }
        now = t.set.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    } else {
        now = undo_values(t, &conf.before);
        for k in t.keep {
            conf.values.remove(*k);
            conf.before.remove(*k);
        }
    }
    let script = sysctl_script(&conf, &now)?;
    Ok(Plan {
        root: true,
        program: "/usr/bin/sh",
        args: vec!["-c".into(), script],
        title: format!("{}: {}", if apply { "Aplicando" } else { "Desfazendo" }, t.title),
    })
}

// ---------------------------------------------------------------- outros ajustes

const PPD: &str = "/usr/bin/powerprofilesctl";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const PROFILES: &[&str] = &["power-saver", "balanced", "performance"];

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).env("LC_ALL", "C").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn unit_enabled(unit: &str) -> bool {
    run(SYSTEMCTL, &["is-enabled", unit]).is_some_and(|s| s == "enabled")
}

fn unit_active(unit: &str) -> bool {
    run(SYSTEMCTL, &["is-active", unit]).is_some_and(|s| s == "active")
}

fn governors() -> Vec<String> {
    let Ok(rd) = fs::read_dir("/sys/devices/system/cpu/cpufreq") else { return Vec::new() };
    let mut v: Vec<String> = rd.flatten().filter_map(|e| read_trim(e.path().join("scaling_governor"))).collect();
    v.sort();
    v.dedup();
    v
}

fn available_governors() -> Vec<String> {
    read_trim("/sys/devices/system/cpu/cpu0/cpufreq/scaling_available_governors").map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_default()
}

/// Nome simples (governor, perfil): letras, números e hífen.
fn simple_word(s: &str) -> bool {
    !s.is_empty() && s.len() < 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Script fixo que troca o governor de todos os núcleos (sem power-profiles-daemon).
pub fn governor_script(gov: &str) -> Result<String, String> {
    if !simple_word(gov) {
        return Err("governor inválido".into());
    }
    Ok(format!("for g in /sys/devices/system/cpu/cpufreq/policy*/scaling_governor; do echo {gov} > \"$g\"; done"))
}

fn base(id: &'static str, group: &'static str, title: &'static str, what: &'static str, effect: &'static str, risk: &'static str) -> Tweak {
    Tweak {
        id,
        group,
        title,
        what,
        effect,
        risk,
        state: "off",
        current: String::new(),
        target: String::new(),
        note: String::new(),
        root: true,
        can_apply: false,
        can_undo: false,
        apply_label: "Aplicar",
        undo_label: "Desfazer",
        restart: false,
    }
}

/// Dá o estado comum: no valor do ajuste e registrado pelo app = "applied"; no valor sem registro = "ok"
/// (ou "cachyos"); fora do valor = "off" e o registro, se havia, é velho.
fn settle(t: &mut Tweak, at_target: bool, rec: &mut Record, cachyos: bool) {
    let ours = rec.before.contains_key(t.id);
    if at_target && ours {
        t.state = "applied";
        t.can_undo = true;
    } else if at_target {
        t.state = if cachyos { "cachyos" } else { "ok" };
    } else {
        rec.before.remove(t.id);
        t.state = "off";
        t.can_apply = true;
    }
}

fn cpu_tweak(rec: &mut Record, prof: &Profile) -> Tweak {
    let mut t = base(
        "cpu",
        "Processador",
        "Processador sempre pronto (modo desempenho)",
        "Deixa a CPU no perfil de desempenho: o clock sobe na hora em vez de esperar a carga aumentar.",
        "Menos engasgos e 1% low melhor em alguns jogos. Gasta mais energia e esquenta um pouco mais parado.",
        "baixo",
    );
    t.target = "performance".into();
    let game_perf = Path::new("/usr/bin/game-performance").exists();
    if let Some(profile) = run(PPD, &["get"]) {
        t.root = false;
        t.current = format!("perfil de energia: {profile}");
        settle(&mut t, profile == "performance", rec, false);
        if t.state == "ok" {
            t.note = "O perfil já está em desempenho.".into();
        }
    } else {
        let govs = governors();
        if govs.is_empty() {
            t.state = "na";
            t.note = "Não deu para ler o governor da CPU (cpufreq).".into();
            return t;
        }
        t.current = format!("governor: {}", govs.join(", "));
        if !available_governors().iter().any(|g| g == "performance") {
            t.state = "na";
            t.note = "O driver da CPU não oferece o modo performance.".into();
            return t;
        }
        settle(&mut t, govs == ["performance"], rec, false);
        t.note = "Sem o power-profiles-daemon, vale até reiniciar.".into();
    }
    if game_perf && prof.cachyos {
        let tip = "Dica do CachyOS: para ligar só durante o jogo, use game-performance %command% nas opções de inicialização do jogo na Steam.";
        t.note = if t.note.is_empty() { tip.into() } else { format!("{} {tip}", t.note) };
    }
    t
}

fn cpu_plan(apply: bool, rec: &mut Record) -> Result<Plan, String> {
    if let Some(profile) = run(PPD, &["get"]) {
        let target = if apply {
            rec.before.entry("cpu".into()).or_insert(profile);
            "performance".to_string()
        } else {
            let prev = rec.before.remove("cpu").filter(|p| PROFILES.contains(&p.as_str()) && p != "performance");
            prev.unwrap_or_else(|| "balanced".into())
        };
        return Ok(Plan { root: false, program: PPD, args: vec!["set".into(), target.clone()], title: format!("Perfil de energia: {target}") });
    }
    let govs = governors();
    let avail = available_governors();
    let target = if apply {
        rec.before.entry("cpu".into()).or_insert_with(|| govs.first().cloned().unwrap_or_default());
        "performance".to_string()
    } else {
        let prev = rec.before.remove("cpu").filter(|g| avail.contains(g) && g != "performance");
        prev.or_else(|| ["schedutil", "powersave"].iter().find(|g| avail.iter().any(|a| a == *g)).map(|g| g.to_string())).ok_or("não há outro governor para voltar")?
    };
    Ok(Plan { root: true, program: "/usr/bin/sh", args: vec!["-c".into(), governor_script(&target)?], title: format!("Governor da CPU: {target}") })
}

fn has_ssd() -> bool {
    let Ok(rd) = fs::read_dir("/sys/block") else { return false };
    rd.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        !n.starts_with("loop") && !n.starts_with("zram") && !n.starts_with("ram") && read_trim(e.path().join("queue/rotational")).as_deref() == Some("0")
    })
}

fn root_mount_options() -> String {
    let m = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    m.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|f| f.len() > 3 && f[1] == "/").map(|f| format!("{} {}", f[2], f[3])).unwrap_or_default()
}

fn fstrim_tweak(rec: &mut Record) -> Tweak {
    let mut t = base(
        "fstrim",
        "Disco",
        "TRIM semanal do SSD",
        "Uma vez por semana avisa o SSD quais blocos foram apagados, para ele reorganizar o espaço livre com calma.",
        "Mantém a velocidade de gravação do SSD com o tempo e ajuda na vida útil. Roda em segundo plano, em segundos.",
        "baixo",
    );
    if !has_ssd() {
        t.state = "na";
        t.note = "Nenhum SSD encontrado.".into();
        return t;
    }
    let on = unit_enabled("fstrim.timer");
    t.current = format!("fstrim.timer {}", if on { "ligado" } else { "desligado" });
    t.target = "fstrim.timer ligado".into();
    settle(&mut t, on, rec, false);
    let opts = root_mount_options();
    if opts.starts_with("btrfs") && opts.contains("discard=async") {
        t.note = "O btrfs daqui também faz TRIM contínuo (discard=async); o semanal é um reforço.".into();
    }
    t
}

fn ananicy_tweak(rec: &mut Record, prof: &Profile) -> Tweak {
    let mut t = base(
        "ananicy",
        "Processador",
        "Prioridade automática de processos (ananicy-cpp)",
        "Dá mais prioridade de CPU e disco a jogos e ao que está na tela, e menos a compilações e tarefas de fundo.",
        "O jogo sofre menos quando algo pesado roda junto (atualização, download, indexação).",
        "baixo",
    );
    if !Path::new("/usr/lib/systemd/system/ananicy-cpp.service").exists() {
        t.state = "na";
        t.note = "Pacote ananicy-cpp não instalado.".into();
        return t;
    }
    let on = unit_enabled("ananicy-cpp.service") && unit_active("ananicy-cpp.service");
    t.current = format!("ananicy-cpp {}", if on { "rodando" } else { "parado" });
    t.target = "ananicy-cpp ligado".into();
    settle(&mut t, on, rec, prof.cachyos);
    if t.state == "cachyos" {
        t.note = "Já vem ligado no CachyOS, com as regras de jogos do projeto.".into();
    }
    t
}

fn multilib() -> bool {
    fs::read_to_string("/etc/pacman.conf").unwrap_or_default().lines().any(|l| l.trim() == "[multilib]")
}

fn installed(pkg: &str) -> bool {
    pkg_version(pkg).is_some()
}

fn gamemode_tweak(rec: &mut Record, prof: &Profile) -> Tweak {
    let mut t = base(
        "gamemode",
        "Jogos",
        "GameMode (Feral)",
        "Programa que, enquanto um jogo roda, põe a CPU em desempenho, aumenta a prioridade do jogo e segura o protetor de tela.",
        "Mesma ideia do Modo de Jogo do Windows. Na Steam, use gamemoderun %command% nas opções de inicialização do jogo.",
        "baixo",
    );
    t.apply_label = "Instalar";
    t.undo_label = "Remover";
    let on = installed("gamemode");
    t.current = if on { "instalado".into() } else { "não instalado".into() };
    t.target = "gamemode instalado".into();
    settle(&mut t, on, rec, false);
    if prof.cachyos && Path::new("/usr/bin/game-performance").exists() {
        t.note = "O CachyOS já traz o game-performance (game-performance %command%), que faz a parte principal. O GameMode é opcional.".into();
    }
    if t.can_apply {
        t.note = format!("{} Instalar também aplica as atualizações pendentes do sistema.", t.note).trim().to_string();
    }
    t
}

fn gamemode_plan(apply: bool, rec: &mut Record) -> Result<Plan, String> {
    if packages::pacman_locked() {
        return Err("outro programa está instalando ou atualizando pacotes agora. Espere ele terminar e tente de novo.".into());
    }
    let mut pkgs = vec!["gamemode".to_string()];
    if multilib() && (apply || installed("lib32-gamemode")) {
        pkgs.push("lib32-gamemode".into());
    }
    let action = if apply {
        rec.before.insert("gamemode".into(), "não instalado".into());
        PkgAction::Install { packages: pkgs }
    } else {
        rec.before.remove("gamemode");
        PkgAction::Remove { packages: pkgs }
    };
    Ok(Plan { root: true, program: packages::PACMAN, args: packages::pacman_args(&action)?, title: if apply { "Instalando o GameMode".into() } else { "Removendo o GameMode".into() } })
}

/// Só informativos: o que o CachyOS já ajusta e não tem o que aplicar.
fn info_tweaks(prof: &Profile) -> Vec<Tweak> {
    let mut v = Vec::new();
    if prof.cachyos_kernel {
        let mut t = base(
            "kernel",
            "Processador",
            "Kernel otimizado para desktop",
            "Kernel do CachyOS: compilado para a sua geração de CPU, com escalonador e temporizador (1000 Hz) pensados para resposta rápida.",
            "Menos latência em jogos e na interface sem precisar mexer em nada.",
            "baixo",
        );
        t.state = "cachyos";
        t.current = prof.kernel.clone();
        let scx = read_trim("/sys/kernel/sched_ext/state").unwrap_or_default();
        t.note = if scx == "enabled" {
            let ops = read_trim("/sys/kernel/sched_ext/root/ops").unwrap_or_default();
            format!("Escalonador sched_ext ativo: {ops}.")
        } else {
            "Usando o escalonador padrão do kernel CachyOS (os escalonadores sched_ext ficam no app scx-manager).".into()
        };
        v.push(t);
    }
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    let zram = swaps.lines().skip(1).find(|l| l.starts_with("/dev/zram")).map(|l| l.split_whitespace().nth(2).and_then(|k| k.parse::<u64>().ok()).unwrap_or(0));
    let mut t = base(
        "zram",
        "Memória",
        "Swap comprimido na RAM (zram)",
        "Em vez de mandar memória para o disco, o sistema a comprime dentro da própria RAM.",
        "Bem mais rápido que swap em disco e quase dobra a memória útil antes de engasgar.",
        "baixo",
    );
    match zram {
        Some(kb) => {
            t.current = format!("zram ativo · {}", crate::util::human_bytes(kb * 1024));
            t.state = if Path::new("/usr/lib/systemd/zram-generator.conf").exists() && !prof.cachyos_settings.is_empty() { "cachyos" } else { "ok" };
        }
        None => {
            t.current = "sem zram".into();
            t.state = "na";
            t.note = "Para usar, instale o zram-generator e configure (não é feito pelo app).".into();
        }
    }
    v.push(t);
    if prof.cachyos && Path::new("/usr/lib/udev/rules.d/60-ioschedulers.rules").exists() {
        let mut t = base(
            "iosched",
            "Disco",
            "Escalonador de disco por tipo",
            "Escolhe a fila de leitura e gravação certa para cada disco: kyber no NVMe, mq-deadline no SSD SATA e bfq no HD.",
            "Leituras do jogo não ficam presas atrás de gravações grandes.",
            "baixo",
        );
        t.state = "cachyos";
        t.current = schedulers();
        v.push(t);
    }
    v
}

fn schedulers() -> String {
    let Ok(rd) = fs::read_dir("/sys/block") else { return String::new() };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("loop") || n.starts_with("zram") {
                return None;
            }
            let s = read_trim(e.path().join("queue/scheduler"))?;
            let cur = s.split_whitespace().find(|w| w.starts_with('['))?.trim_matches(|c| c == '[' || c == ']').to_string();
            Some(format!("{n}: {cur}"))
        })
        .collect();
    v.sort();
    v.join(" · ")
}

/// Todos os ajustes, com o estado atual.
pub fn list() -> Vec<Tweak> {
    let prof = profile();
    let rp = record_path();
    let mut rec = load_record(rp.as_deref());
    let start = rec.clone();
    let env = sys_env(&prof);
    let mut v = vec![cpu_tweak(&mut rec, &prof), ananicy_tweak(&mut rec, &prof), gamemode_tweak(&mut rec, &prof), fstrim_tweak(&mut rec)];
    v.extend(SYS_TWEAKS.iter().map(|t| sys_state(t, &env)));
    v.extend(info_tweaks(&prof));
    if rec != start {
        save_record(rp.as_deref(), &rec);
    }
    const ORDER: &[&str] = &["Processador", "Jogos", "Memória", "Disco", "Kernel"];
    v.sort_by_key(|t| ORDER.iter().position(|g| *g == t.group).unwrap_or(99));
    v
}

/// Plano para aplicar (`apply`) ou desfazer um ajuste. Confere o estado de novo aqui (não confia na interface).
pub fn plan(id: &str, apply: bool) -> Result<Plan, String> {
    let current = list().into_iter().find(|t| t.id == id).ok_or("ajuste desconhecido")?;
    if apply && !current.can_apply {
        return Err("este ajuste não tem o que aplicar agora.".into());
    }
    if !apply && !current.can_undo {
        return Err("só dá para desfazer o que foi aplicado pelo app.".into());
    }
    let rp = record_path();
    let mut rec = load_record(rp.as_deref());
    let p = match id {
        "cpu" => cpu_plan(apply, &mut rec)?,
        "gamemode" => gamemode_plan(apply, &mut rec)?,
        "fstrim" | "ananicy" => {
            let unit = if id == "fstrim" { "fstrim.timer" } else { "ananicy-cpp.service" };
            if apply {
                rec.before.insert(id.into(), "desligado".into());
            } else {
                rec.before.remove(id);
            }
            Plan {
                root: true,
                program: SYSTEMCTL,
                args: vec![if apply { "enable" } else { "disable" }.into(), "--now".into(), unit.into()],
                title: format!("{}: {}", if apply { "Ligando" } else { "Desligando" }, current.title),
            }
        }
        _ => sysctl_plan(id, apply, &sys_env(&profile()))?,
    };
    save_record(rp.as_deref(), &rec);
    Ok(p)
}

// ---------------------------------------------------------------- limpeza

#[derive(Debug, Clone, Serialize)]
pub struct CleanItem {
    /// O que vai de volta em `clean(id, key)`: nome da pasta ou appid.
    pub key: String,
    pub name: String,
    pub size: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cleanup {
    pub id: &'static str,
    pub title: &'static str,
    pub what: &'static str,
    pub risk: &'static str,
    pub root: bool,
    /// Quanto ocupa hoje.
    pub size: u64,
    /// Quanto a limpeza libera (quando dá para saber antes).
    pub reclaim: Option<u64>,
    /// "todo" (tem o que limpar), "ok" (nada a fazer), "cachyos", "na"
    pub state: &'static str,
    pub note: String,
    /// Partes que dá para limpar uma a uma (pastas do ~/.cache, jogos no cache de shaders, órfãos).
    pub items: Vec<CleanItem>,
    /// Limpa tudo de uma vez (sem `key`).
    pub all: bool,
    pub action: &'static str,
}

/// Tamanho de uma pasta (sem seguir links simbólicos).
pub fn dir_size(p: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    rd.flatten().map(|e| dir_size(&e.path())).sum()
}

/// Apaga uma pasta ou arquivo (um link simbólico é apagado sem seguir). Devolve o que liberou.
fn remove_path(p: &Path) -> Result<u64, String> {
    let m = fs::symlink_metadata(p).map_err(|e| e.to_string())?;
    let size = dir_size(p);
    if m.is_dir() {
        fs::remove_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    } else {
        fs::remove_file(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(size)
}

/// Esvazia uma pasta, mantendo a própria pasta.
pub fn clear_dir(dir: &Path) -> Result<u64, String> {
    let Ok(rd) = fs::read_dir(dir) else { return Ok(0) };
    let mut freed = 0;
    for e in rd.flatten() {
        freed += remove_path(&e.path())?;
    }
    Ok(freed)
}

/// Apaga `base/name`, desde que `name` seja um filho direto de verdade (sem "/", "..", nem vazio).
pub fn remove_child(base: &Path, name: &str) -> Result<u64, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err("nome inválido".into());
    }
    let p = base.join(name);
    if fs::symlink_metadata(&p).is_err() {
        return Err("não existe mais".into());
    }
    remove_path(&p)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".cache")))
}

/// "disk space saved: 1.43 GiB" (paccache) e "take up 44.5M" (journalctl) para bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches('.');
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let n: f64 = s[..split].parse().ok()?;
    let unit = s[split..].trim();
    let mult = match unit.trim_end_matches("iB").trim_end_matches('B') {
        "" => 1.0,
        "K" | "k" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0f64.powi(4),
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// Linha final do `paccache -d`: "==> finished dry run: 273 candidates (disk space saved: 1.43 GiB)".
pub fn parse_paccache(out: &str) -> u64 {
    out.lines().rev().find_map(|l| l.split_once("disk space saved:").and_then(|(_, r)| parse_size(r.trim_end_matches(')')))).unwrap_or(0)
}

/// "Archived and active journals take up 44.5M in the file system."
pub fn parse_journal_usage(out: &str) -> Option<u64> {
    let r = out.split_once("take up ")?.1;
    parse_size(r.split_whitespace().next()?)
}

const PACCACHE: &str = "/usr/bin/paccache";

fn pacman_cleanup() -> Cleanup {
    let dir = Path::new("/var/cache/pacman/pkg");
    let size = fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0);
    let mut c = Cleanup {
        id: "pacman",
        title: "Cache de pacotes do pacman",
        what: "Cópias de todo pacote que já foi baixado, inclusive versões antigas. Fica a versão instalada de cada um (para reinstalar sem baixar) e sai o resto, inclusive o de programas que você já removeu.",
        risk: "baixo",
        root: true,
        size,
        reclaim: None,
        state: "todo",
        note: String::new(),
        items: Vec::new(),
        all: true,
        action: "Limpar",
    };
    if !Path::new(PACCACHE).exists() {
        c.state = "na";
        c.note = "Precisa do paccache (pacote pacman-contrib).".into();
        return c;
    }
    let dry = |args: &[&str]| Command::new(PACCACHE).args(args).env("LC_ALL", "C").output().map(|o| parse_paccache(&String::from_utf8_lossy(&o.stdout))).unwrap_or(0);
    let r = dry(&["-dk1"]) + dry(&["-duk0"]);
    c.reclaim = Some(r);
    if r == 0 {
        c.state = "ok";
    }
    c.note = "Depois disso, voltar um pacote para a versão anterior precisa baixá-la de novo.".into();
    c
}

fn orphans() -> Vec<String> {
    run("/usr/bin/pacman", &["-Qdtq"]).map(|s| s.lines().map(String::from).filter(|p| packages::valid_package(p)).collect()).unwrap_or_default()
}

fn orphans_cleanup() -> Cleanup {
    let list = orphans();
    let mut c = Cleanup {
        id: "orphans",
        title: "Pacotes órfãos",
        what: "Pacotes que entraram como dependência de algo que já foi removido e não servem mais a nenhum programa.",
        risk: "médio",
        root: true,
        size: 0,
        reclaim: None,
        state: if list.is_empty() { "ok" } else { "todo" },
        note: "Confira a lista: às vezes um pacote opcional que você usa direto aparece aqui.".into(),
        items: list.iter().map(|p| CleanItem { key: p.clone(), name: p.clone(), size: 0, detail: String::new() }).collect(),
        all: true,
        action: "Remover",
    };
    if list.is_empty() {
        c.note = String::new();
    }
    c
}

fn journal_limit() -> Option<(String, String)> {
    for d in ["/etc/systemd/journald.conf.d", "/run/systemd/journald.conf.d", "/usr/lib/systemd/journald.conf.d"] {
        let Ok(rd) = fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let text = fs::read_to_string(e.path()).unwrap_or_default();
            if let Some(v) = text.lines().find_map(|l| l.trim().strip_prefix("SystemMaxUse=")) {
                return Some((e.path().display().to_string(), v.trim().to_string()));
            }
        }
    }
    None
}

fn journal_cleanup(prof: &Profile) -> Cleanup {
    let size = run("/usr/bin/journalctl", &["--disk-usage"]).and_then(|s| parse_journal_usage(&s)).unwrap_or(0);
    let mut c = Cleanup {
        id: "journal",
        title: "Registro do sistema (journal)",
        what: "Histórico de mensagens do sistema e dos serviços. Apaga o mais antigo e deixa no máximo 50 MB.",
        risk: "baixo",
        root: true,
        size,
        reclaim: Some(size.saturating_sub(50 * 1024 * 1024)),
        state: if size > 64 * 1024 * 1024 { "todo" } else { "ok" },
        note: String::new(),
        items: Vec::new(),
        all: true,
        action: "Limpar",
    };
    if let Some((file, v)) = journal_limit() {
        if file.contains("cachyos") || (prof.cachyos && file.starts_with("/usr/lib")) {
            c.state = if c.state == "ok" { "cachyos" } else { c.state };
            c.note = format!("O CachyOS já limita o registro a {v}.");
        } else {
            c.note = format!("Limite configurado: {v} ({file}).");
        }
    }
    c
}

fn thumbnails_cleanup() -> Cleanup {
    let dir = cache_dir().map(|c| c.join("thumbnails"));
    let size = dir.as_deref().map(dir_size).unwrap_or(0);
    Cleanup {
        id: "thumbnails",
        title: "Miniaturas de imagens e vídeos",
        what: "Prévias que o gerenciador de arquivos guarda. São recriadas sozinhas quando você abre a pasta de novo.",
        risk: "baixo",
        root: false,
        size,
        reclaim: Some(size),
        state: if size > 0 { "todo" } else { "ok" },
        note: String::new(),
        items: Vec::new(),
        all: true,
        action: "Limpar",
    }
}

/// Pasta do ~/.cache -> nome do processo que a usa (para avisar "feche antes").
const CACHE_OWNERS: &[(&str, &str, &str)] = &[
    ("google-chrome", "chrome", "Google Chrome"),
    ("chromium", "chromium", "Chromium"),
    ("mozilla", "firefox", "Firefox"),
    ("BraveSoftware", "brave", "Brave"),
    ("spotify", "spotify", "Spotify"),
    ("discord", "Discord", "Discord"),
    ("yay", "", "yay (AUR)"),
    ("paru", "", "paru (AUR)"),
    ("aur", "", "pacotes do AUR"),
    ("pip", "", "pip (Python)"),
    ("nvidia", "", "shaders da NVIDIA (OpenGL/CUDA)"),
    ("mesa_shader_cache", "", "shaders do Mesa"),
    ("mesa_shader_cache_db", "", "shaders do Mesa"),
    ("thumbnails", "", "miniaturas"),
    ("tracker3", "localsearch-3", "índice de busca do GNOME"),
    ("fontconfig", "", "fontes"),
];

fn running_comms() -> Vec<String> {
    let Ok(rd) = fs::read_dir("/proc") else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| read_trim(e.path().join("comm")))
        .collect()
}

/// Pastas do cache do usuário, maiores primeiro.
pub fn cache_items(dir: &Path, running: &[String]) -> Vec<CleanItem> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<CleanItem> = rd
        .flatten()
        .map(|e| {
            let key = e.file_name().to_string_lossy().into_owned();
            let owner = CACHE_OWNERS.iter().find(|(d, _, _)| *d == key);
            let mut detail = owner.map(|o| o.2.to_string()).unwrap_or_default();
            if let Some((_, proc_name, app)) = owner.filter(|o| !o.1.is_empty()) {
                if running.iter().any(|c| c == proc_name) {
                    detail = format!("{app} está aberto: feche antes de limpar");
                }
            }
            CleanItem { name: key.clone(), size: dir_size(&e.path()), detail, key }
        })
        .filter(|i| i.size > 0 && i.key != "thumbnails" && i.key != "reset-explode")
        .collect();
    v.sort_by_key(|i| std::cmp::Reverse(i.size));
    v
}

fn user_cache_cleanup() -> Cleanup {
    let dir = cache_dir();
    let items = dir.as_deref().map(|d| cache_items(d, &running_comms())).unwrap_or_default();
    let size = items.iter().map(|i| i.size).sum();
    Cleanup {
        id: "cache",
        title: "Cache dos seus programas (~/.cache)",
        what: "Arquivos temporários que cada programa guarda para abrir mais rápido. Dá para apagar: o programa recria o que precisar.",
        risk: "baixo",
        root: false,
        size,
        reclaim: None,
        state: if size > 0 { "todo" } else { "ok" },
        note: "Limpe pasta por pasta. Navegador fechado perde só o cache, não senhas nem histórico.".into(),
        items: items.into_iter().take(12).collect(),
        all: false,
        action: "Limpar",
    }
}

/// Bibliotecas da Steam: a pasta principal e as do libraryfolders.vdf.
pub fn steam_libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    let vdf = fs::read_to_string(steam_root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
    for l in vdf.lines() {
        let parts: Vec<&str> = l.split('"').collect();
        // linha: \t\t"path"\t\t"/mnt/jogos/SteamLibrary"
        if parts.len() >= 4 && parts[1] == "path" {
            libs.push(PathBuf::from(parts[3].replace("\\\\", "\\")));
        }
    }
    let mut seen = Vec::new();
    libs.into_iter()
        .filter(|p| {
            let c = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            let new = !seen.contains(&c) && c.join("steamapps").is_dir();
            seen.push(c);
            new
        })
        .collect()
}

fn acf_name(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines().find_map(|l| {
        let parts: Vec<&str> = l.split('"').collect();
        (parts.len() >= 4 && parts[1] == "name").then(|| parts[3].to_string())
    })
}

/// Cache de shaders por jogo (appid), em todas as bibliotecas.
pub fn shader_items(libs: &[PathBuf]) -> Vec<(CleanItem, PathBuf)> {
    let mut v = Vec::new();
    for lib in libs {
        let Ok(rd) = fs::read_dir(lib.join("steamapps/shadercache")) else { continue };
        for e in rd.flatten() {
            let appid = e.file_name().to_string_lossy().into_owned();
            if !appid.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let size = dir_size(&e.path());
            if size == 0 {
                continue;
            }
            let name = acf_name(&lib.join(format!("steamapps/appmanifest_{appid}.acf"))).unwrap_or_else(|| format!("App {appid}"));
            v.push((CleanItem { key: appid, name, size, detail: lib.display().to_string() }, e.path()));
        }
    }
    v.sort_by_key(|i| std::cmp::Reverse(i.0.size));
    v
}

fn steam_root() -> Option<PathBuf> {
    let h = home()?;
    [h.join(".local/share/Steam"), h.join(".steam/steam")].into_iter().find(|p| p.join("steamapps").is_dir())
}

fn steam_running(running: &[String]) -> bool {
    running.iter().any(|c| c == "steam" || c == "steamwebhelper")
}

fn shader_cleanup() -> Cleanup {
    let libs = steam_root().map(|r| steam_libraries(&r)).unwrap_or_default();
    let items: Vec<CleanItem> = shader_items(&libs).into_iter().map(|(i, _)| i).collect();
    let size = items.iter().map(|i| i.size).sum();
    let mut c = Cleanup {
        id: "shaders",
        title: "Cache de shaders da Steam",
        what: "Shaders já compilados de cada jogo. Apagar libera espaço, mas o jogo recompila na próxima vez e pode engasgar nos primeiros minutos.",
        risk: "médio",
        root: false,
        size,
        reclaim: Some(size),
        state: if size > 0 { "todo" } else { "ok" },
        note: "Só vale a pena para jogos que você desinstalou ou quando um jogo começa a dar erro gráfico depois de atualizar o driver.".into(),
        items,
        all: true,
        action: "Limpar",
    };
    if libs.is_empty() {
        c.state = "na";
        c.note = "Steam não encontrada.".into();
    } else if steam_running(&running_comms()) {
        c.note = format!("A Steam está aberta: feche antes de limpar. {}", c.note);
    }
    c
}

/// Todas as limpezas, com o tamanho de agora.
pub fn cleanups() -> Vec<Cleanup> {
    let prof = profile();
    vec![pacman_cleanup(), orphans_cleanup(), journal_cleanup(&prof), thumbnails_cleanup(), user_cache_cleanup(), shader_cleanup()]
}

/// Limpeza com root: devolve o plano para a janela rodar.
pub fn clean_plan(id: &str) -> Result<Plan, String> {
    match id {
        "pacman" => {
            if !Path::new(PACCACHE).exists() {
                return Err("instale o pacman-contrib para limpar o cache.".into());
            }
            Ok(Plan {
                root: true,
                program: "/usr/bin/sh",
                args: vec!["-c".into(), format!("{PACCACHE} -rk1 && {PACCACHE} -ruk0")],
                title: "Limpando o cache do pacman".into(),
            })
        }
        "orphans" => {
            if packages::pacman_locked() {
                return Err("outro programa está mexendo nos pacotes agora. Espere ele terminar.".into());
            }
            // a lista é lida de novo aqui, nunca vem da interface
            let list = orphans();
            if list.is_empty() {
                return Err("nenhum pacote órfão.".into());
            }
            Ok(Plan { root: true, program: packages::PACMAN, args: packages::pacman_args(&PkgAction::Remove { packages: list })?, title: "Removendo pacotes órfãos".into() })
        }
        "journal" => Ok(Plan { root: true, program: "/usr/bin/journalctl", args: vec!["--vacuum-size=50M".into()], title: "Limpando o registro do sistema".into() }),
        _ => Err("esta limpeza não precisa de administrador".into()),
    }
}

/// Limpeza no nível do usuário (sem root), feita aqui mesmo. Devolve quanto liberou.
pub fn clean_user(id: &str, key: Option<&str>) -> Result<u64, String> {
    match id {
        "thumbnails" => clear_dir(&cache_dir().ok_or("pasta de cache não encontrada")?.join("thumbnails")),
        "cache" => {
            let key = key.ok_or("escolha a pasta")?;
            if key == "reset-explode" {
                return Err("pasta do próprio app".into());
            }
            remove_child(&cache_dir().ok_or("pasta de cache não encontrada")?, key)
        }
        "shaders" => {
            if steam_running(&running_comms()) {
                return Err("feche a Steam antes de limpar o cache de shaders.".into());
            }
            let libs = steam_root().map(|r| steam_libraries(&r)).unwrap_or_default();
            let mut freed = 0;
            for (item, path) in shader_items(&libs) {
                if key.is_none_or(|k| k == item.key) {
                    freed += remove_path(&path)?;
                }
            }
            Ok(freed)
        }
        _ => Err("limpeza desconhecida".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("re-tweaks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn env(current: &[(&str, u64)]) -> SysEnv {
        SysEnv {
            current: current.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            conf: SysctlConf::default(),
            sources: HashMap::new(),
            zram: false,
            cachyos_zram_rule: false,
            cachyos_kernel: false,
        }
    }

    #[test]
    fn arquivo_sysctl_ida_e_volta() {
        let mut c = SysctlConf::default();
        c.values.insert("vm.swappiness".into(), 10);
        c.before.insert("vm.swappiness".into(), 60);
        let text = render_conf(&c);
        assert_eq!(parse_conf(&text), c);
        // chave fora da lista e valor que não é número são ignorados
        let bad = parse_conf("kernel.core_pattern = |/bin/sh\nvm.swappiness = abc\nnet.ipv4.ip_forward = 1\n");
        assert!(bad.values.is_empty());
    }

    #[test]
    fn origem_de_cada_chave() {
        let files = vec![
            ("/etc/sysctl.d".into(), "70-cachyos-settings.conf".into(), "vm.swappiness = 30\n".into()),
            ("/usr/lib/sysctl.d".into(), "10-arch.conf".into(), "vm.max_map_count = 1048576\n".into()),
            ("/usr/lib/sysctl.d".into(), "70-cachyos-settings.conf".into(), "vm.swappiness = 100\n".into()),
            ("/usr/lib/sysctl.d".into(), "50-default.conf".into(), "# vm.swappiness = 1\n-kernel/nmi_watchdog = 1\n".into()),
        ];
        let s = sysctl_sources(&files);
        // o de /etc esconde o de /usr/lib com o mesmo nome
        assert_eq!(s["vm.swappiness"], ("/etc/sysctl.d/70-cachyos-settings.conf".into(), "30".into()));
        assert_eq!(s["vm.max_map_count"].0, "/usr/lib/sysctl.d/10-arch.conf");
        assert_eq!(s["kernel.nmi_watchdog"].1, "1");
    }

    #[test]
    fn estados_dos_ajustes_de_sysctl() {
        let t = SYS_TWEAKS.iter().find(|t| t.id == "vfs_cache").unwrap();
        let mut e = env(&[("vm.vfs_cache_pressure", 100)]);
        assert_eq!(sys_state(t, &e).state, "off");
        assert!(sys_state(t, &e).can_apply);

        e.current.insert("vm.vfs_cache_pressure".into(), 50);
        e.sources.insert("vm.vfs_cache_pressure".into(), ("/usr/lib/sysctl.d/70-cachyos-settings.conf".into(), "50".into()));
        let s = sys_state(t, &e);
        assert_eq!((s.state, s.can_apply, s.can_undo), ("cachyos", false, false));

        e.conf.values.insert("vm.vfs_cache_pressure".into(), 50);
        assert_eq!(sys_state(t, &e).state, "applied");
        assert!(sys_state(t, &e).can_undo);

        // com zram, swappiness alto é o certo
        let sw = SYS_TWEAKS.iter().find(|t| t.id == "swappiness").unwrap();
        let mut z = env(&[("vm.swappiness", 150)]);
        z.zram = true;
        z.cachyos_zram_rule = true;
        assert_eq!(sys_state(sw, &z).state, "cachyos");
        assert!(!sys_state(sw, &z).can_apply);

        // chave que o kernel não tem
        let sl = SYS_TWEAKS.iter().find(|t| t.id == "split_lock").unwrap();
        assert_eq!(sys_state(sl, &env(&[])).state, "na");
    }

    #[test]
    fn plano_de_sysctl_guarda_o_antes_e_desfaz() {
        let mut e = env(&[("vm.dirty_bytes", 0), ("vm.dirty_background_bytes", 0), ("vm.dirty_ratio", 20), ("vm.dirty_background_ratio", 10)]);
        let p = sysctl_plan("dirty", true, &e).unwrap();
        assert!(p.root);
        assert_eq!(p.program, "/usr/bin/sh");
        let script = &p.args[1];
        assert!(script.contains("# antes: vm.dirty_ratio = 20"));
        assert!(script.contains("vm.dirty_bytes = 268435456"));
        assert!(script.contains("sysctl -w vm.dirty_bytes=268435456"));

        // depois de aplicado, o arquivo tem os valores; desfazer volta as razões (sem gravar 0 em *_bytes)
        let body = script.split("<<'RESET_EOF'\n").nth(1).unwrap().split("RESET_EOF").next().unwrap();
        e.conf = parse_conf(body);
        let u = sysctl_plan("dirty", false, &e).unwrap();
        let s = &u.args[1];
        assert!(s.contains(&format!("rm -f {SYSCTL_FILE}")));
        assert!(s.contains("sysctl -w vm.dirty_ratio=20"));
        assert!(s.contains("sysctl -w vm.dirty_background_ratio=10"));
        assert!(!s.contains("vm.dirty_bytes=0"));
    }

    #[test]
    fn desfazer_um_mantem_os_outros() {
        let mut e = env(&[("vm.swappiness", 60), ("vm.vfs_cache_pressure", 100)]);
        e.conf.values.insert("vm.swappiness".into(), 10);
        e.conf.before.insert("vm.swappiness".into(), 60);
        let p = sysctl_plan("vfs_cache", true, &e).unwrap();
        let s = &p.args[1];
        assert!(s.contains("vm.swappiness = 10") && s.contains("vm.vfs_cache_pressure = 50"));
        // com os dois no arquivo, desfazer um regrava o arquivo só com o outro
        let body = s.split("<<'RESET_EOF'\n").nth(1).unwrap().split("RESET_EOF").next().unwrap();
        e.conf = parse_conf(body);
        let u = sysctl_plan("swappiness", false, &e).unwrap();
        let s = &u.args[1];
        assert!(s.contains("sysctl -w vm.swappiness=60"));
        assert!(!s.contains("rm -f") && !s.contains("vm.swappiness = 10") && s.contains("vm.vfs_cache_pressure = 50"));
        assert!(s.contains("# antes: vm.vfs_cache_pressure = 100"));
        assert!(sysctl_plan("kernel.core_pattern", true, &e).is_err());
    }

    #[test]
    fn script_recusa_chave_de_fora() {
        let mut c = SysctlConf::default();
        c.values.insert("kernel.core_pattern".into(), 1);
        assert!(sysctl_script(&c, &[]).is_err());
        assert!(sysctl_script(&SysctlConf::default(), &[("vm.swappiness; reboot".into(), 1)]).is_err());
        assert!(governor_script("performance").unwrap().contains("echo performance >"));
        assert!(governor_script("x; reboot").is_err());
    }

    #[test]
    fn tamanhos_do_paccache_e_journal() {
        assert_eq!(parse_paccache("==> finished dry run: 273 candidates (disk space saved: 1.43 GiB)\n"), (1.43 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(parse_paccache("==> no candidate packages found for pruning\n"), 0);
        assert_eq!(parse_journal_usage("Archived and active journals take up 44.5M in the file system."), Some((44.5 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("2K"), Some(2048));
    }

    #[test]
    fn limpeza_de_pastas_do_usuario() {
        let d = tmp("cache");
        fs::create_dir_all(d.join("google-chrome/Default")).unwrap();
        fs::write(d.join("google-chrome/Default/a"), vec![0u8; 3000]).unwrap();
        fs::create_dir_all(d.join("pip")).unwrap();
        fs::write(d.join("pip/b"), vec![0u8; 100]).unwrap();
        fs::write(d.join("solto.bin"), vec![0u8; 10]).unwrap();
        // link para fora: apagar o link não pode apagar o alvo
        let fora = tmp("fora");
        fs::write(fora.join("importante"), b"x").unwrap();
        std::os::unix::fs::symlink(&fora, d.join("link")).unwrap();

        let items = cache_items(&d, &["chrome".to_string()]);
        assert_eq!(items[0].key, "google-chrome");
        assert!(items[0].detail.contains("feche"));
        assert_eq!(items[0].size, 3000);

        assert!(remove_child(&d, "../fora").is_err());
        assert!(remove_child(&d, "..").is_err());
        assert_eq!(remove_child(&d, "google-chrome").unwrap(), 3000);
        assert!(!d.join("google-chrome").exists());
        remove_child(&d, "link").unwrap();
        assert!(fora.join("importante").exists());

        assert_eq!(clear_dir(&d).unwrap(), 110);
        assert!(d.exists() && fs::read_dir(&d).unwrap().next().is_none());
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(fora).unwrap();
    }

    #[test]
    fn shaders_da_steam_por_jogo() {
        let root = tmp("steam");
        let lib2 = tmp("steamlib");
        fs::create_dir_all(root.join("steamapps/shadercache/730")).unwrap();
        fs::write(root.join("steamapps/shadercache/730/f"), vec![0u8; 500]).unwrap();
        fs::write(root.join("steamapps/appmanifest_730.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n}\n").unwrap();
        fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", root.display(), lib2.display()),
        )
        .unwrap();
        fs::create_dir_all(lib2.join("steamapps/shadercache/1174180")).unwrap();
        fs::write(lib2.join("steamapps/shadercache/1174180/f"), vec![0u8; 900]).unwrap();
        fs::create_dir_all(lib2.join("steamapps/shadercache/naoéappid")).unwrap();

        let libs = steam_libraries(&root);
        assert_eq!(libs.len(), 2);
        let items = shader_items(&libs);
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].0.key.as_str(), items[0].0.size), ("1174180", 900));
        assert_eq!(items[0].0.name, "App 1174180");
        assert_eq!(items[1].0.name, "Counter-Strike 2");
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(lib2).unwrap();
    }

    #[test]
    fn registro_descarta_o_que_ficou_velho() {
        let mut rec = Record::default();
        rec.before.insert("fstrim".into(), "desligado".into());
        let mut t = base("fstrim", "Disco", "", "", "", "baixo");
        settle(&mut t, true, &mut rec, false);
        assert_eq!((t.state, t.can_undo), ("applied", true));
        let mut t = base("fstrim", "Disco", "", "", "", "baixo");
        settle(&mut t, false, &mut rec, false);
        assert_eq!((t.state, t.can_apply), ("off", true));
        assert!(rec.before.is_empty());
        let mut t = base("ananicy", "Processador", "", "", "", "baixo");
        settle(&mut t, true, &mut rec, true);
        assert_eq!(t.state, "cachyos");
    }
}
