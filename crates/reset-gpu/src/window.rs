//! Janela do teste (winit: Wayland nativo, X11 como reserva). O laço de render é nosso: a cada frame os eventos
//! pendentes são processados sem esperar (`pump_app_events` com tempo zero), como o PeekMessage do app do Windows.
//! ESC ou fechar a janela pedem a parada; SIGINT/SIGTERM também (o app pode cancelar e ainda receber o resultado).

use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Fullscreen, Window, WindowId};

/// Ligado pelo tratador de SIGINT/SIGTERM (só um store atômico, seguro dentro de um tratador de sinal).
static SIGNAL_STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNAL_STOP.store(true, Ordering::Relaxed);
}

pub struct Win {
    el: EventLoop<()>,
    st: State,
}

struct State {
    title: String,
    size: (u32, u32),
    fullscreen: bool,
    window: Option<Window>,
    stop: bool,
    resized: bool,
    error: Option<String>,
}

impl ApplicationHandler for State {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // app_id igual ao do app: o GNOME agrupa a janela com o ícone do RESET EXPLODE.
        let mut attrs = Window::default_attributes().with_title(&self.title).with_name("reset-explode", "reset-gpu");
        if self.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        } else {
            // Cabe na tela (85% do maior monitor), mantendo a proporção da resolução de render.
            let (w, h) = (self.size.0 as f64, self.size.1 as f64);
            let (mw, mh) = el
                .available_monitors()
                .map(|m| m.size())
                .max_by_key(|s| s.width as u64 * s.height as u64)
                .map(|s| (s.width as f64, s.height as f64))
                .unwrap_or((1920.0, 1080.0));
            let scale = (mw * 0.85 / w).min(mh * 0.85 / h).min(1.0);
            attrs = attrs.with_inner_size(PhysicalSize::new((w * scale) as u32, (h * scale) as u32));
        }
        match el.create_window(attrs) {
            Ok(w) => self.window = Some(w),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => self.stop = true,
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                self.stop = true
            }
            WindowEvent::Resized(_) => self.resized = true,
            _ => {}
        }
    }
}

impl Win {
    /// Abre a janela e espera ela existir. `w`×`h` é a resolução de render (a janela pode ser menor).
    pub fn open(title: &str, w: u32, h: u32, fullscreen: bool) -> Result<Win> {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        unsafe {
            libc::signal(libc::SIGINT, handler);
            libc::signal(libc::SIGTERM, handler);
        }
        let el = EventLoop::new().context("não foi possível abrir uma janela (sessão gráfica Wayland/X11 indisponível)")?;
        let mut win = Win {
            el,
            st: State {
                title: title.to_string(),
                size: (w, h),
                fullscreen,
                window: None,
                stop: false,
                resized: false,
                error: None,
            },
        };
        let t0 = Instant::now();
        while win.st.window.is_none() {
            if let Some(e) = win.st.error.take() {
                bail!("não foi possível criar a janela: {e}");
            }
            if t0.elapsed() > Duration::from_secs(10) {
                bail!("a janela não abriu em 10 s");
            }
            win.pump_wait(Duration::from_millis(5));
        }
        Ok(win)
    }

    fn pump_wait(&mut self, timeout: Duration) {
        if let PumpStatus::Exit(_) = self.el.pump_app_events(Some(timeout), &mut self.st) {
            self.st.stop = true;
        }
    }

    /// Processa os eventos pendentes sem esperar.
    pub fn pump(&mut self) {
        self.pump_wait(Duration::ZERO);
    }

    pub fn window(&self) -> &Window {
        self.st.window.as_ref().expect("janela criada em Win::open")
    }

    /// ESC, fechar a janela ou SIGINT/SIGTERM.
    pub fn stop_requested(&self) -> bool {
        self.st.stop || SIGNAL_STOP.load(Ordering::Relaxed)
    }

    pub fn take_resized(&mut self) -> bool {
        std::mem::take(&mut self.st.resized)
    }

    /// Tamanho atual da área de desenho, em pixels.
    pub fn size(&self) -> (u32, u32) {
        let s = self.window().inner_size();
        (s.width, s.height)
    }

    pub fn set_title(&self, t: &str) {
        self.window().set_title(t);
    }
}
