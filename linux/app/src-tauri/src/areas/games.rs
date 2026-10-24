//! Área "games": Biblioteca, FPS estimado e Gargalo. A lógica fica em `reset_core::games`; aqui só os comandos.
//!
//! Eventos:
//! - "games-fps-row"           {gen, res, upscaling, name, fps, low, rating | error}   cada jogo respondido pela HowManyFPS
//! - "games-bottleneck"        {elapsed, total, count, game, limiter, title}   uma vez por segundo durante a sessão
//! - "games-bottleneck-done"   {cancelled, report}

use reset_core::games::bottleneck::{self, LoadSample, Report, Session};
use reset_core::games::library::{self, Art, Game};
use reset_core::games::{self, howmanyfps, launch};
use reset_core::sensors::SensorMonitor;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const RESOLUTIONS: &[&str] = &["1920x1080", "2560x1440", "3840x2160"];
const PRESETS: &[&str] = &["Baixo", "Médio", "Alto", "Ultra"];
const UPSCALING: &[&str] = &["Native", "Quality", "Performance"];

pub async fn dispatch(app: &AppHandle, method: &str, args: Value) -> Result<Value, String> {
    let app = app.clone();
    let method = method.to_string();
    // tudo aqui lê disco, /proc ou rede: fora do thread da janela
    tauri::async_runtime::spawn_blocking(move || run(&app, &method, &args)).await.map_err(|e| e.to_string())?
}

fn run(app: &AppHandle, method: &str, a: &Value) -> Result<Value, String> {
    match method {
        // ---------------- biblioteca
        "library" => {
            let lib = library::scan();
            remember(&lib.games);
            Ok(json!({
                "games": lib.games,
                "stores": lib.stores,
                "tools": library::tools(),
                "steam_running": library::steam_running(),
                "with_profile": launch::keys_with_profile(),
            }))
        }
        "arts" => {
            let kind = Art::parse(&arg(a, "kind")?).ok_or("tipo de arte inválido")?;
            let keys = a.get("keys").and_then(|v| v.as_array()).ok_or("faltou: keys")?;
            let mut out = serde_json::Map::new();
            for k in keys.iter().filter_map(|k| k.as_str()).take(64) {
                out.insert(k.to_string(), library::art(k, kind).map(Value::from).unwrap_or(Value::Null));
            }
            Ok(Value::Object(out))
        }
        "profile" => {
            let g = find_cached(&arg(a, "key")?)?;
            let p = launch::profile(&g.key);
            Ok(json!({
                "profile": p,
                "line": launch::build(&p, g.proton()),
                "presets": launch::EXTRA_PRESETS,
                "current": if g.is_steam() { library::steam_launch_options(&g.id) } else { None },
            }))
        }
        "profile_set" => {
            let g = find_cached(&arg(a, "key")?)?;
            let p: launch::LaunchProfile = serde_json::from_value(a.get("profile").cloned().unwrap_or(Value::Null)).map_err(|e| format!("perfil inválido: {e}"))?;
            launch::save_profile(&g.key, &p)?;
            Ok(json!({ "line": launch::build(&p, g.proton()) }))
        }
        "play" => {
            let g = find_game(&arg(a, "key")?)?;
            let uri = library::launch_uri(&g).ok_or("não sei abrir jogos desta loja")?;
            library::xdg_open(&uri)?;
            let via = match g.store.as_str() {
                "Steam" => "pela Steam",
                "Heroic" => "pelo Heroic",
                _ => "pelo Lutris",
            };
            Ok(json!({ "message": format!("Abrindo {} {via}…", g.name) }))
        }
        "open_dir" => {
            let g = find_game(&arg(a, "key")?)?;
            if !std::path::Path::new(&g.install_dir).is_dir() {
                return Err("a pasta do jogo não existe mais".into());
            }
            library::xdg_open(&g.install_dir)?;
            Ok(Value::Null)
        }

        // ---------------- FPS estimado
        "fps_init" => {
            let lib = cached_games();
            Ok(json!({
                "machine": games::machine(),
                "configured": howmanyfps::load_key().is_some(),
                "installed": lib.iter().map(|g| g.name.clone()).collect::<Vec<_>>(),
            }))
        }
        "fps_estimate" => {
            let (res, preset, up) = fps_options(a)?;
            let names: Vec<String> = cached_games().into_iter().map(|g| g.name).collect();
            Ok(json!(games::estimate_all(&games::machine(), &res, &preset, &up, &names)))
        }
        "fps_fetch" => fps_fetch(app, a),
        "fps_cancel" => {
            FPS_GEN.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        }
        "fps_key_set" => {
            howmanyfps::save_key(&arg(a, "key")?)?;
            Ok(json!({ "configured": howmanyfps::load_key().is_some() }))
        }

        // ---------------- gargalo
        "bn_init" => {
            let m = games::machine();
            Ok(json!({ "balance": bottleneck::balance(m.gpu_idx, m.cpu_idx), "machine": m }))
        }
        "bn_live" => {
            let snap = {
                let mut mon = LIVE.lock().unwrap();
                mon.get_or_insert_with(SensorMonitor::new).poll()
            };
            let s = LoadSample::from_snapshot(&snap);
            let l = s.classify();
            Ok(json!({
                "sample": s,
                "limiter": l,
                "title": bottleneck::title(l),
                "advice": bottleneck::advice(l),
                "alert": bottleneck::is_alert(l),
                "game": running_game(),
            }))
        }
        "bn_start" => bn_start(app, a),
        "bn_stop" => {
            if let Some(s) = SESSION.lock().unwrap().as_ref() {
                s.cancel.store(true, Ordering::SeqCst);
            }
            Ok(Value::Null)
        }
        "bn_status" => {
            let running = SESSION.lock().unwrap().as_ref().map(|s| json!({ "elapsed": s.started.elapsed().as_secs(), "total": s.total_s }));
            Ok(json!({ "running": running, "last": LAST_REPORT.lock().unwrap().clone() }))
        }
        _ => Err(format!("games: método desconhecido: {method}")),
    }
}

fn arg(a: &Value, k: &str) -> Result<String, String> {
    a.get(k).and_then(|v| v.as_str()).map(str::to_string).ok_or_else(|| format!("faltou: {k}"))
}

// ---------------------------------------------------------------- biblioteca em memória

/// Última leitura da biblioteca: serve para saber qual jogo está rodando sem reler tudo a cada segundo.
static LIB: Mutex<Option<(Instant, Vec<Game>)>> = Mutex::new(None);

fn remember(g: &[Game]) {
    *LIB.lock().unwrap() = Some((Instant::now(), g.to_vec()));
}

fn cached_games() -> Vec<Game> {
    if let Some((at, g)) = LIB.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return g.clone();
        }
    }
    let g = library::scan().games;
    remember(&g);
    g
}

/// Só abre o que está de fato na biblioteca (a chave vem da interface; nada de texto livre virar comando).
fn find_game(key: &str) -> Result<Game, String> {
    let lib = library::scan().games;
    remember(&lib);
    lib.into_iter().find(|g| g.key == key).ok_or_else(|| "jogo não encontrado na biblioteca (atualize a lista)".into())
}

/// Para o perfil basta a leitura recente (o perfil é do app; nada é aberto nem executado).
fn find_cached(key: &str) -> Result<Game, String> {
    match cached_games().into_iter().find(|g| g.key == key) {
        Some(g) => Ok(g),
        None => find_game(key),
    }
}

static RUNNING: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

/// Jogo rodando agora; a varredura do /proc vale por 3 s.
fn running_game() -> Option<String> {
    let mut r = RUNNING.lock().unwrap();
    if let Some((at, name)) = r.as_ref() {
        if at.elapsed() < Duration::from_secs(3) {
            return name.clone();
        }
    }
    let name = library::running_game(&cached_games());
    *r = Some((Instant::now(), name.clone()));
    name
}

// ---------------------------------------------------------------- HowManyFPS

/// Cada busca ganha um número; uma busca nova (ou trocar de opção) faz a anterior parar.
static FPS_GEN: AtomicU64 = AtomicU64::new(0);

fn fps_options(a: &Value) -> Result<(String, String, String), String> {
    let pick = |k: &str, allowed: &[&str], def: &str| {
        let v = a.get(k).and_then(|v| v.as_str()).unwrap_or(def);
        if allowed.contains(&v) { Ok(v.to_string()) } else { Err(format!("{k} inválido: {v}")) }
    };
    Ok((pick("res", RESOLUTIONS, "1920x1080")?, pick("preset", PRESETS, "Alto")?, pick("upscaling", UPSCALING, "Native")?))
}

fn fps_fetch(app: &AppHandle, a: &Value) -> Result<Value, String> {
    let (res, _, up) = fps_options(a)?;
    let key = howmanyfps::load_key().ok_or("chave da API não configurada")?;
    let gen = FPS_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let m = games::machine();
    let mut ok = 0;
    for g in games::catalog::GAMES.iter() {
        if FPS_GEN.load(Ordering::SeqCst) != gen {
            return Ok(json!({ "gen": gen, "cancelled": true, "ok": ok }));
        }
        match howmanyfps::get(&key, &m.cpu_short, &m.gpu_short, g.name, &res, &up) {
            Ok(r) => {
                ok += 1;
                let _ = app.emit("games-fps-row", json!({ "gen": gen, "res": res, "upscaling": up, "name": g.name, "fps": r.avg_fps, "low": r.min1_fps, "rating": games::catalog::rating(r.avg_fps) }));
            }
            Err(e) => {
                let _ = app.emit("games-fps-row", json!({ "gen": gen, "res": res, "upscaling": up, "name": g.name, "error": e }));
                // chave recusada ou cota esgotada: não adianta insistir nos outros jogos
                if e.contains("chave") || e.contains("cota") || e.contains("conexão") {
                    return Ok(json!({ "gen": gen, "ok": ok, "total": games::catalog::GAMES.len(), "stopped": e }));
                }
            }
        }
    }
    Ok(json!({ "gen": gen, "ok": ok, "total": games::catalog::GAMES.len() }))
}

// ---------------------------------------------------------------- sessão de gargalo

struct Running {
    cancel: Arc<AtomicBool>,
    started: Instant,
    total_s: u64,
}

static SESSION: Mutex<Option<Running>> = Mutex::new(None);
static LAST_REPORT: Mutex<Option<Report>> = Mutex::new(None);
static LIVE: Mutex<Option<SensorMonitor>> = Mutex::new(None);

fn bn_start(app: &AppHandle, a: &Value) -> Result<Value, String> {
    let minutes = a.get("minutes").and_then(|v| v.as_u64()).unwrap_or(1);
    if ![1, 3, 5].contains(&minutes) {
        return Err("duração inválida".into());
    }
    let mut slot = SESSION.lock().unwrap();
    if slot.is_some() {
        return Err("já tem uma análise em andamento".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let total_s = minutes * 60;
    *slot = Some(Running { cancel: cancel.clone(), started: Instant::now(), total_s });
    *LAST_REPORT.lock().unwrap() = None;
    let app = app.clone();
    std::thread::spawn(move || {
        // monitor próprio: a carga da CPU é calculada entre duas leituras desta thread, a cada 1 s certinho
        let mut mon = SensorMonitor::new();
        mon.poll();
        let mut session = Session::new();
        let start = Instant::now();
        let lib = cached_games();
        let mut game: Option<String> = None;
        let mut cancelled = false;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if cancel.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            let s = LoadSample::from_snapshot(&mon.poll());
            let l = s.classify();
            session.add(s);
            if session.count() % 5 == 1 {
                game = library::running_game(&lib);
                if let Some(g) = &game {
                    session.note_game(g);
                }
            }
            let elapsed = start.elapsed().as_secs();
            let _ = app.emit(
                "games-bottleneck",
                json!({ "elapsed": elapsed, "total": total_s, "count": session.count(), "game": game, "limiter": l, "title": bottleneck::title(l) }),
            );
            if elapsed >= total_s {
                break;
            }
        }
        let report = (!cancelled).then(|| session.report());
        *LAST_REPORT.lock().unwrap() = report.clone();
        *SESSION.lock().unwrap() = None;
        let _ = app.emit("games-bottleneck-done", json!({ "cancelled": cancelled, "report": report }));
    });
    Ok(json!({ "total": total_s }))
}
