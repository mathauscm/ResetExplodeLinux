//! Cliente da API da HowManyFPS (POST howmanyfps.com/api → FPS médio e 1% low medidos em sessões reais).
//! Portado do HowManyFpsClient.cs. Credenciais são pedidas por e-mail (info@howmanyfps.com).
//!
//! No Windows a chave era cifrada com DPAPI; aqui fica em ~/.config/reset-explode/howmanyfps.key com
//! permissão 0600 (só o seu usuário lê). Como o plano tem cota, as respostas ficam em cache por 7 dias
//! (~/.cache/reset-explode/howmanyfps.json). A requisição usa o `curl` do sistema, com a chave e o corpo
//! passados pela entrada padrão (não aparecem na lista de processos).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const ENDPOINT: &str = "https://howmanyfps.com/api";
const CACHE_TTL_S: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FpsResult {
    pub avg_fps: f64,
    pub min1_fps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    avg_fps: f64,
    min1_fps: f64,
    /// Unix, segundos.
    fetched: u64,
}

fn key_path() -> Option<PathBuf> {
    Some(super::config_dir()?.join("howmanyfps.key"))
}

fn cache_path() -> Option<PathBuf> {
    Some(super::cache_dir()?.join("howmanyfps.json"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A chave só pode ter letras, números e `-_.:=+/` (formatos comuns de token). Barra qualquer coisa que
/// pudesse quebrar o cabeçalho HTTP ou o arquivo de configuração do curl.
pub fn valid_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 512 && k.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:=+/".contains(c))
}

pub fn load_key() -> Option<String> {
    let k = fs::read_to_string(key_path()?).ok()?;
    let k = k.trim().to_string();
    valid_key(&k).then_some(k)
}

/// Grava a chave (0600). Vazia apaga o arquivo.
pub fn save_key(key: &str) -> Result<(), String> {
    write_key(&key_path().ok_or("pasta de configuração não encontrada")?, key)
}

fn write_key(p: &std::path::Path, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return match fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("não deu para apagar a chave: {e}")),
            _ => Ok(()),
        };
    }
    if !valid_key(key) {
        return Err("a chave tem caracteres inválidos (use só a chave enviada pela HowManyFPS)".into());
    }
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p).map_err(|e| e.to_string())?;
    // o arquivo pode já existir com outra permissão: garante 0600
    f.set_permissions(fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    f.write_all(key.as_bytes()).map_err(|e| e.to_string())
}

fn cache_key(cpu: &str, gpu: &str, game: &str, res: &str, up: &str) -> String {
    format!("{cpu}|{gpu}|{game}|{res}|{up}")
}

fn load_cache() -> HashMap<String, CacheEntry> {
    cache_path().and_then(|p| fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_cache(c: &HashMap<String, CacheEntry>) {
    let Some(p) = cache_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string(c) {
        let _ = fs::write(p, t);
    }
}

fn fresh(e: &CacheEntry, now: u64) -> bool {
    now.saturating_sub(e.fetched) < CACHE_TTL_S
}

/// Resposta do cache, se tiver uma com menos de 7 dias.
pub fn cached(cpu: &str, gpu: &str, game: &str, res: &str, up: &str) -> Option<FpsResult> {
    let c = load_cache();
    let e = c.get(&cache_key(cpu, gpu, game, res, up))?;
    fresh(e, now()).then_some(FpsResult { avg_fps: e.avg_fps, min1_fps: e.min1_fps })
}

/// Consulta (ou devolve do cache). Erro já vem em texto legível.
pub fn get(key: &str, cpu: &str, gpu: &str, game: &str, res: &str, up: &str) -> Result<FpsResult, String> {
    if let Some(r) = cached(cpu, gpu, game, res, up) {
        return Ok(r);
    }
    if !valid_key(key) {
        return Err("chave da API não configurada".into());
    }
    let body = serde_json::json!({ "cpu": cpu, "gpu": gpu, "game": game, "resolution": res, "upscaling": up }).to_string();
    let (code, text) = post(key, &body)?;
    if !(200..300).contains(&code) {
        return Err(match code {
            401 | 403 => "chave recusada pela API".into(),
            404 => "jogo ou hardware não encontrado na base".into(),
            429 => "cota de requisições esgotada".into(),
            _ => format!("HTTP {code}"),
        });
    }
    let r = parse_response(&text)?;
    let mut c = load_cache();
    c.insert(cache_key(cpu, gpu, game, res, up), CacheEntry { avg_fps: r.avg_fps, min1_fps: r.min1_fps, fetched: now() });
    save_cache(&c);
    Ok(r)
}

fn parse_response(text: &str) -> Result<FpsResult, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "resposta inválida".to_string())?;
    let avg = v.get("avgFps").and_then(|x| x.as_f64()).ok_or("resposta sem avgFps")?;
    Ok(FpsResult { avg_fps: avg, min1_fps: v.get("min1Fps").and_then(|x| x.as_f64()).unwrap_or(0.0) })
}

/// Texto entre aspas no arquivo de configuração do curl.
fn curl_quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn curl_config(key: &str, body: &str) -> String {
    // O formato exato de autenticação vem com as credenciais; enviamos os dois cabeçalhos mais comuns.
    [
        format!("url = {}", curl_quote(ENDPOINT)),
        "request = \"POST\"".into(),
        "header = \"Content-Type: application/json\"".into(),
        format!("header = {}", curl_quote(&format!("Authorization: Bearer {key}"))),
        format!("header = {}", curl_quote(&format!("x-api-key: {key}"))),
        format!("data = {}", curl_quote(body)),
    ]
    .join("\n")
        + "\n"
}

/// (código HTTP, corpo)
fn post(key: &str, body: &str) -> Result<(u16, String), String> {
    let mut child = Command::new("curl")
        .args(["-sS", "--max-time", "15", "--proto", "=https", "-w", "\n%{http_code}", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("não deu para rodar o curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(curl_config(key, body).as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("sem conexão com a API ({})", err.trim().trim_start_matches("curl: ")));
    }
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", text.as_str()));
    Ok((code.trim().parse().unwrap_or(0), body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation() {
        assert!(valid_key("abc-123_XYZ.token=="));
        assert!(!valid_key(""));
        assert!(!valid_key("abc\"\nurl = \"http://mal\""));
        assert!(!valid_key("com espaço"));
    }

    #[test]
    fn key_file_is_private() {
        let dir = std::env::temp_dir().join(format!("reset-hmf-{}", std::process::id()));
        let p = dir.join("sub/howmanyfps.key");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "velha").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_key(&p, "  nova-chave_1 \n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "nova-chave_1");
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_key(&p, "com espaço").is_err());
        write_key(&p, "").unwrap();
        assert!(!p.exists());
        write_key(&p, "").unwrap(); // apagar de novo não é erro
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn curl_config_escapes_body() {
        let body = r#"{"game":"Baldur's Gate 3","cpu":"x\"y"}"#;
        let c = curl_config("k1", body);
        assert!(c.contains(r#"data = "{\"game\":\"Baldur's Gate 3\",\"cpu\":\"x\\\"y\"}""#));
        assert!(c.contains("header = \"x-api-key: k1\""));
        assert_eq!(c.lines().count(), 6);
    }

    #[test]
    fn response_and_cache_age() {
        let r = parse_response(r#"{"avgFps": 143.5, "min1Fps": 98}"#).unwrap();
        assert_eq!(r, FpsResult { avg_fps: 143.5, min1_fps: 98.0 });
        assert_eq!(parse_response(r#"{"avgFps": 80}"#).unwrap().min1_fps, 0.0);
        assert!(parse_response(r#"{"erro": 1}"#).is_err());
        let e = CacheEntry { avg_fps: 1.0, min1_fps: 1.0, fetched: 1_000_000 };
        assert!(fresh(&e, 1_000_000 + CACHE_TTL_S - 1));
        assert!(!fresh(&e, 1_000_000 + CACHE_TTL_S));
    }
}
