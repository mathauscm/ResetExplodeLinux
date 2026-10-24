//! Iluminação RGB: mesmos efeitos do app do Windows (`MainWindow.Integ.cs`), calculados aqui e enviados
//! LED a LED pelo OpenRGB. Um thread próprio roda o efeito a ~30 quadros/s enquanto estiver ligado.

pub mod openrgb;

use crate::sensors::SensorMonitor;
use openrgb::OpenRgb;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct LightDevice {
    pub id: String,
    pub name: String,
    pub detail: String,
    /// "Placa-mãe", "Mouse", "Teclado"...
    pub kind: String,
    pub zones: Vec<Zone>,
    /// Posição horizontal (0–1) de cada LED; o tamanho é a quantidade de LEDs (o arco-íris anda por ela).
    #[serde(skip)]
    pub led_x: Vec<f64>,
}

/// Zona de um dispositivo (no OpenRGB): "JRGB1", "JRAINBOW1", "Fury Slot 1"...
#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    pub name: String,
    pub leds: u32,
    pub min: u32,
    pub max: u32,
    /// Conector endereçável (ARGB) cujo tamanho o usuário informa: o controlador não sabe o que está ligado nele.
    pub resizable: bool,
    /// Ordem dos canais que o que está ligado nesta zona espera (salva na configuração do app).
    pub order: ColorOrder,
}

/// Ordem em que um LED lê os três canais. Fans e fitas ARGB baratos costumam ser GRB: mandar rosa em RGB
/// para eles acende outra cor. A gente troca os canais antes de enviar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorOrder {
    #[default]
    Rgb,
    Grb,
    Brg,
    Rbg,
    Gbr,
    Bgr,
}

impl ColorOrder {
    /// Bytes a enviar para o LED mostrar `c`: na posição i vai o canal que o LED lê ali.
    pub fn apply(self, [r, g, b]: [u8; 3]) -> [u8; 3] {
        match self {
            ColorOrder::Rgb => [r, g, b],
            ColorOrder::Grb => [g, r, b],
            ColorOrder::Brg => [b, r, g],
            ColorOrder::Rbg => [r, b, g],
            ColorOrder::Gbr => [g, b, r],
            ColorOrder::Bgr => [b, g, r],
        }
    }
}

/// Tamanho que um conector ARGB recebe quando ninguém informou: em 0 ele apaga o que estiver ligado.
/// LEDs a mais são ignorados pelo controlador, então errar para cima não faz mal.
pub const DEFAULT_ARGB_LEDS: u32 = 60;

/// O que o usuário configurou por zona ("dispositivo|zona"), guardado em ~/.config/reset-explode/lighting.json.
/// O OpenRGB esquece os tamanhos quando reinicia; o app reaplica a cada conexão.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LightConfig {
    #[serde(default)]
    zone_sizes: HashMap<String, u32>,
    #[serde(default)]
    color_order: HashMap<String, ColorOrder>,
    /// Último efeito escolhido. Em `selected` ficam os NOMES dos dispositivos (o índice do OpenRGB pode mudar).
    #[serde(default)]
    settings: Option<LightSettings>,
    /// O efeito estava ligado: reaplica sozinho quando o app (ou o login) conecta.
    #[serde(default)]
    active: bool,
}

impl LightConfig {
    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("reset-explode").join("lighting.json"))
    }

    fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Some(p) = Self::path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, t);
        }
    }
}

/// "3:MSI B550M..." -> "MSI B550M..."
fn device_name(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, n)| n)
}

fn zone_key(device: &LightDevice, zone: &Zone) -> String {
    format!("{}|{}", device.name, zone.name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Static,
    Breath,
    Rainbow,
    CpuTemp,
    GpuTemp,
    CpuLoad,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightSettings {
    pub effect: Effect,
    pub color: [u8; 3],
    /// 0–100
    pub brightness: u8,
    /// 1–10
    pub speed: f64,
    /// Ids dos dispositivos que o efeito controla.
    pub selected: Vec<String>,
}

impl Default for LightSettings {
    fn default() -> Self {
        Self { effect: Effect::Static, color: [255, 0, 0], brightness: 100, speed: 5.0, selected: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LightStatus {
    pub connected: bool,
    pub running: bool,
    pub message: String,
    /// Texto do efeito ao vivo: "CPU 52 °C".
    pub live: String,
    pub devices: Vec<LightDevice>,
    pub settings: LightSettings,
}

struct State {
    conn: Option<OpenRgb>,
    devices: Vec<LightDevice>,
    settings: LightSettings,
    running: bool,
    message: String,
    live: String,
    /// Dispositivos já em modo Direct nesta sessão.
    acquired: HashSet<String>,
    /// Últimas cores enviadas (e quando), para não reenviar a mesma coisa a cada quadro.
    last: HashMap<String, (Vec<[u8; 3]>, Instant)>,
    config: LightConfig,
    /// Seleção salva (por nome) esperando a primeira conexão para virar ids.
    pending_selection: Option<Vec<String>>,
    /// "Identificar": acende só esta zona (dispositivo, zona) em branco até o instante indicado.
    identify: Option<(String, usize, Instant)>,
}

impl State {
    /// Guarda efeito, seleção e se está ligado, para reaplicar depois (inclusive com o app fechado e reaberto).
    fn persist(&mut self) {
        let mut saved = self.settings.clone();
        saved.selected = saved.selected.iter().map(|id| device_name(id).to_string()).collect();
        self.config.settings = Some(saved);
        self.config.active = self.running;
        self.config.save();
    }

    /// Põe nos dispositivos a ordem de cor salva de cada zona.
    fn annotate(&mut self) {
        for d in &mut self.devices {
            for i in 0..d.zones.len() {
                let key = zone_key(d, &d.zones[i]);
                d.zones[i].order = self.config.color_order.get(&key).copied().unwrap_or_default();
            }
        }
    }
}

#[derive(Clone)]
pub struct Lighting {
    state: Arc<Mutex<State>>,
}

impl Default for Lighting {
    fn default() -> Self {
        Self::new()
    }
}

impl Lighting {
    pub fn new() -> Self {
        let config = LightConfig::load();
        let mut settings = config.settings.clone().unwrap_or_default();
        let pending_selection = config.settings.as_ref().map(|_| std::mem::take(&mut settings.selected));
        let state = Arc::new(Mutex::new(State {
            conn: None,
            devices: Vec::new(),
            settings,
            running: false,
            message: "desconectado".into(),
            live: String::new(),
            acquired: HashSet::new(),
            last: HashMap::new(),
            config,
            pending_selection,
            identify: None,
        }));
        let s = state.clone();
        thread::Builder::new().name("rgb".into()).spawn(move || engine(s)).expect("thread de iluminação");
        Self { state }
    }

    pub fn connect(&self, host: &str, port: u16) -> LightStatus {
        self.pause();
        let result = OpenRgb::connect(host, port);
        let mut st = self.state.lock().unwrap();
        match result {
            Ok((mut c, mut devices)) => {
                // conectores ARGB: tamanho salvo pelo usuário ou, se estiverem em 0, um padrão que não apaga nada
                let mut resized = false;
                for d in &devices {
                    for (i, z) in d.zones.iter().enumerate().filter(|(_, z)| z.resizable) {
                        let saved = st.config.zone_sizes.get(&zone_key(d, z)).copied();
                        let want = saved.unwrap_or(if z.leds == 0 { DEFAULT_ARGB_LEDS } else { z.leds }).clamp(z.min, z.max);
                        if want != z.leds && c.resize_zone(&d.id, i as u32, want).is_ok() {
                            resized = true;
                        }
                    }
                }
                if resized {
                    if let Ok(list) = c.list() {
                        devices = list;
                    }
                }
                st.message = format!("conectado (protocolo {}) · {} dispositivo(s) com LEDs.", c.protocol(), devices.len());
                // seleção salva (por nome); senão mantém a dos que continuam existindo; na primeira vez, todos
                if let Some(names) = st.pending_selection.take() {
                    st.settings.selected = devices.iter().filter(|d| names.contains(&d.name)).map(|d| d.id.clone()).collect();
                }
                let ids: HashSet<&str> = devices.iter().map(|d| d.id.as_str()).collect();
                st.settings.selected.retain(|id| ids.contains(id.as_str()));
                if st.settings.selected.is_empty() {
                    st.settings.selected = devices.iter().map(|d| d.id.clone()).collect();
                }
                st.devices = devices;
                st.annotate();
                st.conn = Some(c);
                // o efeito estava ligado da última vez: volta sozinho
                if st.config.active && !st.devices.is_empty() {
                    st.running = true;
                    st.last.clear();
                    st.message = "efeito salvo reaplicado.".into();
                }
            }
            Err(e) => {
                st.conn = None;
                st.devices.clear();
                st.message = e;
            }
        }
        status(&st)
    }

    /// Conecta sem o usuário ter que fazer nada: se o servidor do OpenRGB não estiver no ar (depois de um
    /// reboot, por exemplo), abre ele e espera a detecção. O servidor abre a porta em ~1 s, mas a lista de
    /// dispositivos só chega quando a varredura de USB e I²C termina, por isso tenta até aparecer algo.
    pub fn ensure(&self, host: &str, port: u16) -> LightStatus {
        {
            let st = self.state.lock().unwrap();
            if st.conn.is_some() && !st.devices.is_empty() {
                return status(&st);
            }
        }
        let mut s = self.connect(host, port);
        if s.connected && !s.devices.is_empty() {
            return s;
        }
        let local = matches!(host, "127.0.0.1" | "localhost" | "::1");
        if !s.connected && local && in_path("openrgb") {
            if let Err(e) = launch_openrgb(port) {
                self.state.lock().unwrap().message = e.clone();
                return self.status();
            }
        } else if !s.connected {
            return s;
        }
        for _ in 0..20 {
            thread::sleep(Duration::from_secs(1));
            s = self.connect(host, port);
            if s.connected && !s.devices.is_empty() {
                break;
            }
        }
        if s.connected && s.devices.is_empty() {
            self.state.lock().unwrap().message = "o OpenRGB está no ar, mas não encontrou dispositivos com LEDs.".into();
            return self.status();
        }
        s
    }

    /// Informa quantos LEDs há num conector ARGB e relê os dispositivos (a quantidade de LEDs muda).
    pub fn resize_zone(&self, device: &str, zone: u32, size: u32) -> LightStatus {
        let mut st = self.state.lock().unwrap();
        let st = &mut *st;
        let Some(conn) = st.conn.as_mut() else {
            st.message = "conecte ao OpenRGB primeiro.".into();
            return status(st);
        };
        match conn.resize_zone(device, zone, size).and_then(|_| conn.list().map_err(|e| e.to_string())) {
            Ok(devices) => {
                if let Some(d) = devices.iter().find(|d| d.id == device) {
                    if let Some(z) = d.zones.get(zone as usize) {
                        st.config.zone_sizes.insert(zone_key(d, z), size);
                        st.config.save();
                    }
                }
                st.devices = devices;
                st.annotate();
                st.last.clear();
            }
            Err(e) => st.message = format!("não deu para mudar o tamanho da zona: {e}"),
        }
        status(st)
    }

    /// Muda (e salva) a ordem dos canais de uma zona.
    pub fn set_order(&self, device: &str, zone: u32, order: ColorOrder) -> LightStatus {
        let mut st = self.state.lock().unwrap();
        let key = st.devices.iter().find(|d| d.id == device).and_then(|d| d.zones.get(zone as usize).map(|z| zone_key(d, z)));
        if let Some(key) = key {
            if order == ColorOrder::Rgb {
                st.config.color_order.remove(&key);
            } else {
                st.config.color_order.insert(key, order);
            }
            st.config.save();
            st.annotate();
            st.last.clear();
        }
        status(&st)
    }

    pub fn apply(&self, settings: LightSettings) -> LightStatus {
        let mut st = self.state.lock().unwrap();
        // dispositivos que saíram da seleção voltam ao efeito de fábrica na hora
        if st.running {
            let keep: HashSet<&String> = settings.selected.iter().collect();
            let gone: Vec<String> = st.acquired.iter().filter(|id| !keep.contains(id)).cloned().collect();
            for id in gone {
                if let Some(c) = st.conn.as_mut() {
                    c.release(&id);
                }
                st.acquired.remove(&id);
            }
        }
        st.settings = settings;
        st.last.clear();
        st.persist();
        status(&st)
    }

    pub fn start(&self) -> LightStatus {
        let mut st = self.state.lock().unwrap();
        if st.conn.is_none() {
            st.message = "conecte ao OpenRGB primeiro.".into();
        } else {
            st.running = true;
            st.last.clear();
            st.message = "efeito ligado.".into();
            st.persist();
        }
        status(&st)
    }

    /// Acende só uma zona, em branco, por alguns segundos (o resto apaga): mostra qual peça é qual
    /// e se todos os LEDs dela respondem.
    pub fn identify(&self, device: &str, zone: usize) -> LightStatus {
        let mut st = self.state.lock().unwrap();
        if st.conn.is_none() {
            st.message = "conecte ao OpenRGB primeiro.".into();
        } else {
            st.identify = Some((device.to_string(), zone, Instant::now() + Duration::from_secs(4)));
            st.running = true;
            st.last.clear();
        }
        status(&st)
    }

    /// Para o efeito e devolve cada dispositivo ao modo que tinha antes (efeito salvo na placa/periférico).
    /// Fica salvo como desligado: não volta sozinho na próxima abertura.
    pub fn stop(&self) -> LightStatus {
        let s = self.pause();
        let mut st = self.state.lock().unwrap();
        st.persist();
        drop(st);
        s
    }

    fn pause(&self) -> LightStatus {
        let mut st = self.state.lock().unwrap();
        let st = &mut *st;
        if st.running {
            if let Some(c) = st.conn.as_mut() {
                for id in st.acquired.drain() {
                    c.release(&id);
                }
            }
            st.running = false;
            st.live.clear();
            st.message = "controle devolvido: os LEDs voltaram ao efeito de antes.".into();
        }
        st.acquired.clear();
        status(st)
    }

    pub fn status(&self) -> LightStatus {
        status(&self.state.lock().unwrap())
    }
}

fn status(st: &State) -> LightStatus {
    LightStatus {
        connected: st.conn.is_some(),
        running: st.running,
        message: st.message.clone(),
        live: st.live.clone(),
        devices: st.devices.clone(),
        settings: st.settings.clone(),
    }
}

/// O que falta no sistema para os LEDs funcionarem. A tela de iluminação mostra isso como uma lista de passos.
#[derive(Debug, Clone, Serialize)]
pub struct LightEnv {
    pub openrgb_installed: bool,
    /// Regras udev do OpenRGB: sem elas o OpenRGB só vê os dispositivos rodando como root.
    pub openrgb_udev: bool,
    /// /dev/i2c-*: necessário para memórias RGB e algumas placas de vídeo.
    pub i2c_dev: bool,
    pub openrazer_installed: bool,
}

pub fn environment() -> LightEnv {
    let udev_dirs = ["/usr/lib/udev/rules.d", "/etc/udev/rules.d", "/lib/udev/rules.d"];
    let openrgb_udev = udev_dirs.iter().any(|d| {
        std::fs::read_dir(d).map(|it| it.flatten().any(|e| e.file_name().to_string_lossy().contains("openrgb"))).unwrap_or(false)
    });
    let i2c_dev = std::fs::read_dir("/dev").map(|it| it.flatten().any(|e| e.file_name().to_string_lossy().starts_with("i2c-"))).unwrap_or(false);
    LightEnv {
        openrgb_installed: in_path("openrgb"),
        openrgb_udev,
        i2c_dev,
        openrazer_installed: in_path("openrazer-daemon"),
    }
}

/// Abre o OpenRGB só como servidor SDK (sem janela), sem esperar ele terminar. Com a interface gráfica ele
/// mostra um aviso de I²C que trava a inicialização até alguém fechar.
pub fn launch_openrgb(port: u16) -> Result<(), String> {
    std::process::Command::new("openrgb")
        .args(["--server", "--server-port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            // recolhe o processo quando o OpenRGB fechar, para não ficar zumbi
            thread::spawn(move || child.wait());
        })
        .map_err(|e| format!("não deu para abrir o OpenRGB: {e}"))
}

/// Atalho em ~/.config/autostart que roda `<exe> --apply-lights` ao entrar no sistema: o OpenRGB não
/// guarda as cores entre reinícios, então o app reaplica as que o usuário escolheu.
fn autostart_path() -> Option<PathBuf> {
    LightConfig::path().and_then(|p| Some(p.parent()?.parent()?.join("autostart").join("reset-explode-lights.desktop")))
}

pub fn autostart_enabled() -> bool {
    autostart_path().is_some_and(|p| p.exists())
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let path = autostart_path().ok_or("pasta de configuração não encontrada")?;
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=RESET EXPLODE (iluminação)\nComment=Reaplica as cores dos LEDs ao entrar no sistema\nExec=\"{}\" --apply-lights\nIcon=reset-explode\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&path, entry).map_err(|e| e.to_string())
}

/// Efeitos que mudam com o tempo precisam do app rodando; os fixos ficam no hardware depois de enviados.
pub fn is_animated(effect: Effect) -> bool {
    !matches!(effect, Effect::Static | Effect::Off)
}

fn in_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

/// Intervalo mínimo entre envios ao OpenRGB por dispositivo (o mesmo do app do Windows).
const MIN_INTERVAL: Duration = Duration::from_millis(25);
/// Reenvia mesmo sem mudança de cor, para o firmware não retomar o efeito próprio.
const KEEPALIVE: Duration = Duration::from_secs(2);

fn engine(state: Arc<Mutex<State>>) {
    let clock = Instant::now();
    let mut sensors = SensorMonitor::new();
    let mut last_poll = Instant::now() - Duration::from_secs(10);
    let mut smooth: Option<f64> = None;
    let mut last_effect = Effect::Static;
    let mut live_value: Option<f64> = None;

    loop {
        thread::sleep(Duration::from_millis(33));
        let mut st = state.lock().unwrap();
        if !st.running || st.conn.is_none() {
            smooth = None;
            continue;
        }
        let s = st.settings.clone();
        if s.effect != last_effect {
            smooth = None;
            last_effect = s.effect;
        }

        // efeitos ligados a sensores: lê 2x por segundo e suaviza (a cor não "pisca" com a oscilação da temperatura)
        let live_effect = matches!(s.effect, Effect::CpuTemp | Effect::GpuTemp | Effect::CpuLoad);
        let mut live_rgb = None;
        if live_effect {
            if last_poll.elapsed() >= Duration::from_millis(500) {
                drop(st);
                let snap = sensors.poll();
                last_poll = Instant::now();
                live_value = match s.effect {
                    Effect::CpuTemp => snap.summary.cpu_temp_c,
                    Effect::GpuTemp => snap.summary.gpu.as_ref().and_then(|g| g.temp_c).map(|v| v as f64),
                    _ => snap.summary.cpu_load_pct,
                };
                st = state.lock().unwrap();
                if !st.running {
                    continue;
                }
            }
            match live_value {
                None => {
                    st.live = "Sensor indisponível.".into();
                    live_rgb = Some([0, 0, 40]);
                }
                Some(raw) => {
                    let v = match smooth {
                        None => raw,
                        Some(p) => p + (raw - p) * 0.08,
                    };
                    smooth = Some(v);
                    let hue = if s.effect == Effect::CpuLoad {
                        st.live = format!("CPU {v:.0}% de carga");
                        120.0 * (1.0 - (v / 100.0).clamp(0.0, 1.0))
                    } else {
                        st.live = format!("{} {v:.0} °C", if s.effect == Effect::CpuTemp { "CPU" } else { "GPU" });
                        240.0 * (1.0 - ((v - 35.0) / 50.0).clamp(0.0, 1.0))
                    };
                    live_rgb = Some(hsv(hue, 1.0, 1.0));
                }
            }
        } else {
            st.live.clear();
        }

        let t = clock.elapsed().as_secs_f64();
        let bright = s.brightness.min(100) as f64 / 100.0;
        let now = Instant::now();
        if st.identify.as_ref().is_some_and(|(_, _, until)| now >= *until) {
            st.identify = None;
            st.last.clear();
        }
        let st = &mut *st;
        let ident = st.identify.clone();
        for d in st.devices.iter().filter(|d| s.selected.contains(&d.id) || ident.as_ref().is_some_and(|(id, _, _)| *id == d.id)) {
            if let Some((_, at)) = st.last.get(&d.id) {
                if now.duration_since(*at) < MIN_INTERVAL {
                    continue;
                }
            }
            // os LEDs do dispositivo vêm zona após zona, na ordem das zonas
            let orders = d.zones.iter().flat_map(|z| std::iter::repeat_n(z.order, z.leds as usize)).chain(std::iter::repeat(ColorOrder::Rgb));
            let mut cols: Vec<[u8; 3]> = d
                .led_x
                .iter()
                .zip(orders)
                .map(|(&x, order)| {
                    let c = effect_color(s.effect, s.color, s.speed, t, x, live_rgb);
                    order.apply(c.map(|v| (v as f64 * bright) as u8))
                })
                .collect();
            if let Some((id, zone, _)) = &ident {
                // identificando: só a zona escolhida em branco, todo o resto apagado
                let start: usize = d.zones.iter().take(*zone).map(|z| z.leds as usize).sum();
                let len = if *id == d.id { d.zones.get(*zone).map_or(0, |z| z.leds as usize) } else { 0 };
                for (i, c) in cols.iter_mut().enumerate() {
                    *c = if i >= start && i < start + len { [255, 255, 255] } else { [0, 0, 0] };
                }
            }
            if let Some((prev, at)) = st.last.get(&d.id) {
                if *prev == cols && now.duration_since(*at) < KEEPALIVE {
                    continue;
                }
            }
            let Some(conn) = st.conn.as_mut() else { break };
            if st.acquired.insert(d.id.clone()) {
                // brilho do próprio modo no máximo (a memória Kingston vem em 80%); o brilho do app é aplicado nas cores
                if let Err(e) = conn.acquire(&d.id).and_then(|_| conn.full_brightness(&d.id)) {
                    st.message = format!("{}: {e}", d.name);
                }
            }
            if let Err(e) = conn.send(&d.id, &cols) {
                // conexão caiu (OpenRGB fechado): para e avisa
                st.message = format!("o OpenRGB parou de responder ({e}). Conecte de novo.");
                st.running = false;
                st.conn = None;
                st.acquired.clear();
                break;
            }
            st.last.insert(d.id.clone(), (cols, now));
        }
    }
}

/// Cor de um LED na posição `x` (0–1) no instante `t`. Mesmas fórmulas do app do Windows.
pub fn effect_color(effect: Effect, color: [u8; 3], speed: f64, t: f64, x: f64, live: Option<[u8; 3]>) -> [u8; 3] {
    match effect {
        Effect::Static => color,
        Effect::Breath => {
            let k = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * (0.1 * speed) * t).cos();
            let k = 0.04 + 0.96 * k;
            color.map(|c| (c as f64 * k) as u8)
        }
        Effect::Rainbow => hsv(x * 360.0 + t * speed * 36.0, 1.0, 1.0),
        Effect::CpuTemp | Effect::GpuTemp | Effect::CpuLoad => live.unwrap_or([0, 0, 0]),
        Effect::Off => [0, 0, 0],
    }
}

pub fn hsv(h: f64, s: f64, v: f64) -> [u8; 3] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|v| ((v + m) * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordem_de_cor() {
        let rosa = [220, 138, 221];
        assert_eq!(ColorOrder::Rgb.apply(rosa), rosa);
        // um LED GRB lê o 1º byte como verde: para mostrar rosa ele precisa receber [138, 220, 221]
        assert_eq!(ColorOrder::Grb.apply(rosa), [138, 220, 221]);
        assert_eq!(ColorOrder::Bgr.apply([1, 2, 3]), [3, 2, 1]);
    }

    #[test]
    fn hsv_primarias() {
        assert_eq!(hsv(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv(120.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv(240.0, 1.0, 1.0), [0, 0, 255]);
        assert_eq!(hsv(-120.0, 1.0, 1.0), [0, 0, 255]);
    }

    /// Precisa do OpenRGB instalado e mexe no hardware: `cargo test -p reset-core -- --ignored`.
    #[test]
    #[ignore]
    fn abre_o_openrgb_e_conecta_sozinho() {
        let l = Lighting::new();
        let s = l.ensure("127.0.0.1", openrgb::DEFAULT_PORT);
        assert!(s.connected, "{}", s.message);
        assert!(!s.devices.is_empty(), "{}", s.message);
    }

    #[test]
    fn respiracao_nunca_apaga_de_vez() {
        let c = effect_color(Effect::Breath, [200, 100, 0], 5.0, 0.0, 0.0, None);
        assert_eq!(c, [8, 4, 0]); // 4% no fundo da respiração
    }
}
