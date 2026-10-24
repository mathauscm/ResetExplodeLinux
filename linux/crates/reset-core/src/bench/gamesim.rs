//! "CPU de jogo", portado do `GameSimBench.cs`: simula o trabalho de CPU de um frame de jogo com 30 mil
//! unidades em dois times. Por frame: (1) thread principal monta a grade espacial (em série, como a maioria
//! das engines); (2) workers em paralelo fazem física, colisão e IA (cada unidade procura inimigos próximos,
//! persegue/foge e troca dano); (3) thread principal roda a lógica de jogo (mortes/renascimento) e prepara a
//! lista de desenho (culling + ordenação por profundidade). Determinístico (semente fixa, estado em buffer duplo).

use rayon::prelude::*;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const ENTITIES: usize = 30_000;
pub const WORLD: f32 = 1000.0;
const GRID_N: usize = 64; // 64×64 células
const CELL: f32 = WORLD / GRID_N as f32;
const RADIUS: f32 = CELL; // raio de percepção
const DT: f32 = 1.0 / 60.0;
/// Lado do mapa que a interface desenha (células de 2 bits: vazio, time A, time B, os dois).
pub const VIEW: usize = 220;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SimResult {
    pub avg_fps: f64,
    pub low1_fps: f64,
    pub main_thread_ms: f64,
    pub parallel_ms: f64,
    pub frames: usize,
    pub seconds: f64,
    /// Nota = FPS médio da simulação.
    pub score: f64,
}

/// Andamento para a interface (lido de outra thread enquanto roda).
#[derive(Default)]
pub struct SimLive {
    pub frames: AtomicU64,
    progress: AtomicU64,
    fps: AtomicU64,
    pub warming: AtomicBool,
    view: Mutex<Vec<u8>>,
}

impl SimLive {
    pub fn progress(&self) -> f64 {
        f64::from_bits(self.progress.load(Ordering::Relaxed))
    }
    pub fn fps(&self) -> f64 {
        f64::from_bits(self.fps.load(Ordering::Relaxed))
    }
    /// Mapa da batalha visto de cima: VIEW × VIEW células, 4 por byte (2 bits cada, da esquerda para a direita).
    pub fn view(&self) -> Vec<u8> {
        self.view.lock().unwrap().clone()
    }
}

#[derive(Clone, Copy, Default)]
struct Out {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    dmg: f32,
}

pub struct GameSim {
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    hp: Vec<f32>,
    dmg: Vec<f32>,
    team: Vec<u8>,
    out: Vec<Out>,
    cell_start: Vec<u32>,
    sorted: Vec<u32>,
    cell_of: Vec<u32>,
    draw: Vec<(f32, u32)>,
    rng: u32,
    frames: u64,
}

impl Default for GameSim {
    fn default() -> Self {
        Self::new()
    }
}

impl GameSim {
    pub fn new() -> Self {
        let mut s = Self {
            x: vec![0.0; ENTITIES],
            y: vec![0.0; ENTITIES],
            vx: vec![0.0; ENTITIES],
            vy: vec![0.0; ENTITIES],
            hp: vec![0.0; ENTITIES],
            dmg: vec![0.0; ENTITIES],
            team: vec![0; ENTITIES],
            out: vec![Out::default(); ENTITIES],
            cell_start: vec![0; GRID_N * GRID_N + 1],
            sorted: vec![0; ENTITIES],
            cell_of: vec![0; ENTITIES],
            draw: Vec::with_capacity(ENTITIES),
            rng: 0,
            frames: 0,
        };
        s.reset();
        s
    }

    /// Roda 2 s de aquecimento (clock subindo, caches) e mede por `duration`. None se cancelado.
    pub fn run(&mut self, duration: Duration, cancel: &AtomicBool, live: &SimLive) -> Option<SimResult> {
        self.reset();
        live.warming.store(true, Ordering::Relaxed);
        let warm = Instant::now();
        let mut last_view = Instant::now();
        while warm.elapsed() < Duration::from_secs(2) && !cancel.load(Ordering::Relaxed) {
            self.step();
            if last_view.elapsed() >= Duration::from_millis(100) {
                self.publish_view(live);
                last_view = Instant::now();
            }
        }
        live.warming.store(false, Ordering::Relaxed);

        let mut frame_times = Vec::with_capacity(20_000);
        let (mut main_ms, mut par_ms) = (0.0, 0.0);
        self.frames = 0;
        let sw = Instant::now();
        let mut fps_window = Instant::now();
        let mut fps_frames = 0;
        while sw.elapsed() < duration && !cancel.load(Ordering::Relaxed) {
            let t0 = Instant::now();
            let (m, p) = self.step();
            frame_times.push(t0.elapsed().as_secs_f64() * 1000.0);
            main_ms += m;
            par_ms += p;
            self.frames += 1;
            live.frames.store(self.frames, Ordering::Relaxed);
            fps_frames += 1;
            if fps_window.elapsed() >= Duration::from_millis(500) {
                live.fps.store((fps_frames as f64 / fps_window.elapsed().as_secs_f64()).to_bits(), Ordering::Relaxed);
                fps_frames = 0;
                fps_window = Instant::now();
            }
            live.progress.store((sw.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0).to_bits(), Ordering::Relaxed);
            if last_view.elapsed() >= Duration::from_millis(100) {
                self.publish_view(live);
                last_view = Instant::now();
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let secs = sw.elapsed().as_secs_f64();
        Some(summarize(&frame_times, main_ms, par_ms, secs))
    }

    fn reset(&mut self) {
        self.rng = 12345;
        for i in 0..ENTITIES {
            self.team[i] = (i & 1) as u8;
            let a = self.team[i] == 0;
            // Times começam em lados opostos e avançam um contra o outro.
            self.x[i] = (if a { 0.05 } else { 0.55 }) * WORLD + self.rnd() * WORLD * 0.4;
            self.y[i] = self.rnd() * WORLD;
            self.vx[i] = (if a { 1.0 } else { -1.0 }) * (20.0 + self.rnd() * 10.0);
            self.vy[i] = (self.rnd() - 0.5) * 10.0;
            self.hp[i] = 100.0;
        }
    }

    /// Um frame. Devolve (ms na thread principal, ms em paralelo).
    fn step(&mut self) -> (f64, f64) {
        let t0 = Instant::now();
        self.build_grid(); // série
        let t1 = Instant::now();
        let chunks = rayon::current_num_threads() * 4;
        let per = ENTITIES.div_ceil(chunks);
        {
            let mut out = std::mem::take(&mut self.out);
            let me = &*self;
            out.par_chunks_mut(per).enumerate().for_each(|(chunk, slice)| {
                for (k, o) in slice.iter_mut().enumerate() {
                    *o = me.simulate(chunk * per + k);
                }
            });
            self.out = out;
        }
        // "troca de buffer": o estado novo passa a ser o atual
        for (i, o) in self.out.iter().enumerate() {
            self.x[i] = o.x;
            self.y[i] = o.y;
            self.vx[i] = o.vx;
            self.vy[i] = o.vy;
            self.dmg[i] = o.dmg;
        }
        let t2 = Instant::now();
        self.game_logic(); // série
        self.prepare_draw(); // série
        let t3 = Instant::now();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        (ms(t1 - t0) + ms(t3 - t2), ms(t2 - t1))
    }

    /// Counting sort das unidades por célula (grade espacial para buscar vizinhos).
    fn build_grid(&mut self) {
        self.cell_start.fill(0);
        let clamp = |v: f32| ((v / CELL) as i32).clamp(0, GRID_N as i32 - 1) as usize;
        for i in 0..ENTITIES {
            let c = clamp(self.y[i]) * GRID_N + clamp(self.x[i]);
            self.cell_of[i] = c as u32;
            self.cell_start[c + 1] += 1;
        }
        for c in 0..GRID_N * GRID_N {
            self.cell_start[c + 1] += self.cell_start[c];
        }
        let mut fill = self.cell_start.clone();
        for i in 0..ENTITIES {
            let c = self.cell_of[i] as usize;
            self.sorted[fill[c] as usize] = i as u32;
            fill[c] += 1;
        }
    }

    /// Física + separação (colisão) + IA de combate de uma unidade. Lê o estado atual, escreve só a própria.
    fn simulate(&self, i: usize) -> Out {
        let (mut x, mut y, mut vx, mut vy) = (self.x[i], self.y[i], self.vx[i], self.vy[i]);
        let team = self.team[i];
        let (mut sep_x, mut sep_y, mut ali_x, mut ali_y, mut coh_x, mut coh_y, mut dmg) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let (mut best_d2, mut enemy_x, mut enemy_y) = (RADIUS * RADIUS, 0f32, 0f32);
        let mut allies = 0;
        let mut enemy = false;

        let c = self.cell_of[i] as usize;
        let (cx, cy) = (c % GRID_N, c / GRID_N);
        for gy in cy.saturating_sub(1)..=(cy + 1).min(GRID_N - 1) {
            for gx in cx.saturating_sub(1)..=(cx + 1).min(GRID_N - 1) {
                let cell = gy * GRID_N + gx;
                for k in self.cell_start[cell]..self.cell_start[cell + 1] {
                    let j = self.sorted[k as usize] as usize;
                    if j == i {
                        continue;
                    }
                    let dx = self.x[j] - x;
                    let dy = self.y[j] - y;
                    let d2 = dx * dx + dy * dy;
                    if d2 > RADIUS * RADIUS || d2 < 1e-6 {
                        continue;
                    }
                    let inv = 1.0 / d2.sqrt();
                    if d2 < 9.0 {
                        // colisão: empurra
                        sep_x -= dx * inv;
                        sep_y -= dy * inv;
                    }
                    if self.team[j] == team {
                        ali_x += self.vx[j];
                        ali_y += self.vy[j];
                        coh_x += dx;
                        coh_y += dy;
                        allies += 1;
                    } else {
                        if d2 < 16.0 {
                            dmg += 4.0 * DT * (1.0 - d2.sqrt() / 4.0); // corpo a corpo
                        }
                        if d2 < best_d2 {
                            best_d2 = d2;
                            enemy_x = dx;
                            enemy_y = dy;
                            enemy = true;
                        }
                    }
                }
            }
        }

        // Decisão de IA: vida baixa → foge do inimigo mais próximo; senão persegue.
        let (mut ax, mut ay) = (sep_x * 60.0, sep_y * 60.0);
        if allies > 0 {
            let n = allies as f32;
            ax += (ali_x / n - vx) * 0.5 + coh_x / n * 0.3;
            ay += (ali_y / n - vy) * 0.5 + coh_y / n * 0.3;
        }
        if enemy {
            let inv = 1.0 / best_d2.sqrt();
            let dir = if self.hp[i] < 30.0 { -1.0 } else { 1.0 };
            ax += dir * enemy_x * inv * 80.0;
            ay += dir * enemy_y * inv * 80.0;
        } else {
            ax += if team == 0 { 25.0 } else { -25.0 }; // sem inimigo por perto: marcha para a base inimiga
        }

        vx += ax * DT;
        vy += ay * DT;
        let sp = (vx * vx + vy * vy).sqrt();
        if sp > 60.0 {
            vx *= 60.0 / sp;
            vy *= 60.0 / sp;
        }
        x += vx * DT;
        y += vy * DT;
        if !(0.0..=WORLD).contains(&x) {
            vx = -vx;
            x = x.clamp(0.0, WORLD);
        }
        if !(0.0..=WORLD).contains(&y) {
            vy = -vy;
            y = y.clamp(0.0, WORLD);
        }
        Out { x, y, vx, vy, dmg }
    }

    /// Lógica de jogo em série: aplica dano; quem morre ou chega à base inimiga ("pontua") renasce na própria base.
    /// O fluxo contínuo nos dois sentidos mantém uma frente de batalha no meio e a carga estável durante o teste.
    fn game_logic(&mut self) {
        for i in 0..ENTITIES {
            self.hp[i] = (self.hp[i] - self.dmg[i] + 0.5 * DT).min(100.0);
            let a = self.team[i] == 0;
            let scored = if a { self.x[i] > WORLD * 0.97 } else { self.x[i] < WORLD * 0.03 };
            if self.hp[i] > 0.0 && !scored {
                continue;
            }
            self.hp[i] = 100.0;
            self.x[i] = (if a { 0.02 } else { 0.98 }) * WORLD;
            self.y[i] = self.rnd() * WORLD;
            self.vx[i] = if a { 25.0 } else { -25.0 };
            self.vy[i] = 0.0;
        }
    }

    /// Preparação do desenho em série: câmera em perspectiva, culling e ordenação de trás para frente.
    fn prepare_draw(&mut self) {
        let cam_x = WORLD / 2.0 + (self.frames as f32 * 0.01).sin() * 200.0;
        let cam_y = -200.0;
        self.draw.clear();
        for i in 0..ENTITIES {
            let dx = self.x[i] - cam_x;
            let dy = self.y[i] - cam_y;
            if dy <= 0.0 || dx.abs() > dy * 1.2 {
                continue; // fora do campo de visão
            }
            self.draw.push((-dy, i as u32)); // mais longe primeiro
        }
        self.draw.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        std::hint::black_box(&self.draw);
    }

    fn publish_view(&self, live: &SimLive) {
        let mut v = vec![0u8; VIEW * VIEW / 4];
        let k = VIEW as f32 / WORLD;
        for i in 0..ENTITIES {
            let px = (self.x[i] * k) as usize;
            let py = (self.y[i] * k) as usize;
            if px < VIEW && py < VIEW {
                let cell = py * VIEW + px;
                v[cell / 4] |= (1 + self.team[i]) << ((cell % 4) * 2);
            }
        }
        *live.view.lock().unwrap() = v;
    }

    fn rnd(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

fn summarize(frame_times: &[f64], main_ms: f64, par_ms: f64, secs: f64) -> SimResult {
    let n = frame_times.len().max(1) as f64;
    let mut sorted = frame_times.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p99 = if sorted.is_empty() { 0.0 } else { sorted[(sorted.len() - 1).min((sorted.len() as f64 * 0.99) as usize)] };
    let avg = if secs > 0.0 { frame_times.len() as f64 / secs } else { 0.0 };
    SimResult {
        avg_fps: avg,
        low1_fps: if p99 > 0.0 { 1000.0 / p99 } else { 0.0 },
        main_thread_ms: main_ms / n,
        parallel_ms: par_ms / n,
        frames: frame_times.len(),
        seconds: secs,
        score: avg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estado_inicial_deterministico() {
        let a = GameSim::new();
        let b = GameSim::new();
        assert_eq!(a.x, b.x);
        assert!(a.x.iter().all(|&x| (0.0..=WORLD).contains(&x)));
        // time A começa à esquerda, time B à direita
        assert!(a.x[0] < 450.0 && a.x[1] > 550.0);
    }

    #[test]
    fn frames_mantem_unidades_no_mundo() {
        let mut s = GameSim::new();
        for _ in 0..5 {
            s.step();
        }
        assert!(s.x.iter().chain(&s.y).all(|v| (0.0..=WORLD).contains(v)));
        assert!(s.hp.iter().all(|&h| h > 0.0 && h <= 100.0));
        let live = SimLive::default();
        s.publish_view(&live);
        let v = live.view();
        assert_eq!(v.len(), VIEW * VIEW / 4);
        assert!(v.iter().any(|&b| b != 0));
    }

    /// Medição curta (2 s de aquecimento + 4 s): `cargo test --release -p reset-core sim_curta -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sim_curta() {
        let r = GameSim::new().run(Duration::from_secs(4), &AtomicBool::new(false), &SimLive::default()).unwrap();
        println!("simulação: {:.0} FPS, 1% low {:.0}, principal {:.2} ms, paralelo {:.2} ms", r.avg_fps, r.low1_fps, r.main_thread_ms, r.parallel_ms);
    }

    #[test]
    fn low1_pelo_percentil_99() {
        let mut t = vec![2.0; 99];
        t.push(10.0);
        let r = summarize(&t, 100.0, 100.0, 1.0);
        assert_eq!(r.frames, 100);
        assert_eq!(r.low1_fps, 100.0);
        assert_eq!(r.avg_fps, 100.0);
    }
}
