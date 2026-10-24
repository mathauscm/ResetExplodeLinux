//! RESET BENCH: renderizador de benchmark no modelo do Cinebench, portado do `BenchRenderer.cs`.
//! Path tracer de CPU, imagem dividida em blocos de 32 px distribuídos entre threads do centro para fora.
//! Monocromático e determinístico: cada pixel tem semente fixa, então a imagem e o volume de trabalho são
//! idênticos em toda execução e iguais aos do app do Windows (mesmo RNG, mesma cena, mesma ordem das contas
//! em ponto flutuante de 32 bits). A nota é amostras/s ÷ 10.000, a mesma conta do C#.

use serde::Serialize;
use std::hint::black_box;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

pub const WIDTH: usize = 960;
pub const HEIGHT: usize = 540;
pub const BUCKET: usize = 32;
/// Amostras por pixel.
pub const SPP: usize = 1024;
const MAX_DEPTH: usize = 6;
/// Amostras de uma passada completa (960 × 540 × 1024).
pub const SAMPLES_PER_PASS: u64 = (WIDTH * HEIGHT * SPP) as u64;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BenchResult {
    pub threads: usize,
    pub seconds: f64,
    pub samples: u64,
    pub passes: u32,
    /// Pontos RESET BENCH: amostras de path tracing por segundo ÷ 10.000.
    pub score: f64,
}

impl BenchResult {
    fn new(threads: usize, seconds: f64, samples: u64, passes: u32) -> Self {
        let score = if seconds > 0.0 { samples as f64 / seconds / 10_000.0 } else { 0.0 };
        Self { threads, seconds, samples, passes, score }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Bucket {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// Blocos da imagem em ordem de renderização: do centro para fora, como o Cinebench (empates ficam na
/// ordem de leitura, igual ao `OrderBy` estável do C#).
pub fn buckets() -> Vec<Bucket> {
    let mut list = Vec::new();
    for y in (0..HEIGHT).step_by(BUCKET) {
        for x in (0..WIDTH).step_by(BUCKET) {
            list.push(Bucket { x, y, w: BUCKET.min(WIDTH - x), h: BUCKET.min(HEIGHT - y) });
        }
    }
    let dist = |b: &Bucket| {
        let dx = b.x as f64 + b.w as f64 / 2.0 - WIDTH as f64 / 2.0;
        let dy = (b.y as f64 + b.h as f64 / 2.0 - HEIGHT as f64 / 2.0) * 1.6;
        dx * dx + dy * dy
    };
    list.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    list
}

// ------------------------------------------------------------------ execução

/// Linhas prontas de um bloco, para a interface desenhar (tons de cinza, 1 byte por pixel).
#[derive(Debug, Clone, Serialize)]
pub struct Tile {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// Estado compartilhado entre as threads que renderizam e quem mostra o andamento.
pub struct RenderState {
    pub buckets: Vec<Bucket>,
    pixels: Mutex<Vec<u8>>,
    /// Bloco em que cada thread está (-1 = livre).
    active: Mutex<Vec<i64>>,
    /// Blocos com linhas novas desde a última leitura: (bloco, primeira linha, linhas prontas).
    dirty: Mutex<Vec<(usize, usize, usize)>>,
    samples: AtomicU64,
    done: AtomicUsize,
    pass: AtomicI64,
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderState {
    pub fn new() -> Self {
        Self {
            buckets: buckets(),
            pixels: Mutex::new(vec![0; WIDTH * HEIGHT]),
            active: Mutex::new(Vec::new()),
            dirty: Mutex::new(Vec::new()),
            samples: AtomicU64::new(0),
            done: AtomicUsize::new(0),
            pass: AtomicI64::new(0),
        }
    }

    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    /// Blocos terminados na passada atual.
    pub fn buckets_done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Passada atual (1, 2, ...); muda quando a imagem recomeça.
    pub fn pass(&self) -> i64 {
        self.pass.load(Ordering::Relaxed)
    }

    /// Blocos que as threads estão renderizando agora.
    pub fn active(&self) -> Vec<Bucket> {
        self.active.lock().unwrap().iter().filter(|&&b| b >= 0).map(|&b| self.buckets[b as usize]).collect()
    }

    /// Junta as linhas novas por bloco e devolve os retângulos com os pixels (a interface pinta só o que mudou).
    pub fn take_tiles(&self) -> Vec<Tile> {
        let dirty = std::mem::take(&mut *self.dirty.lock().unwrap());
        let mut spans: Vec<(usize, usize, usize)> = Vec::new(); // (bloco, de, até)
        for (b, y0, n) in dirty {
            match spans.iter_mut().find(|s| s.0 == b) {
                Some(s) => {
                    s.1 = s.1.min(y0);
                    s.2 = s.2.max(y0 + n);
                }
                None => spans.push((b, y0, y0 + n)),
            }
        }
        let px = self.pixels.lock().unwrap();
        spans
            .into_iter()
            .map(|(b, y0, y1)| {
                let bk = self.buckets[b];
                let mut data = Vec::with_capacity(bk.w * (y1 - y0));
                for y in y0..y1 {
                    data.extend_from_slice(&px[y * WIDTH + bk.x..y * WIDTH + bk.x + bk.w]);
                }
                Tile { x: bk.x, y: y0, w: bk.w, h: y1 - y0, data }
            })
            .collect()
    }

    /// Imagem inteira como está agora (tons de cinza, 960 × 540).
    pub fn image(&self) -> Vec<u8> {
        self.pixels.lock().unwrap().clone()
    }

    /// Renderiza a imagem inteira com N threads (multi e single fazem o mesmo trabalho, como no Cinebench).
    /// Com `min_duration`, repete passadas até completar o tempo (desempenho sustentado). Para sozinho
    /// quando `cancel` vira true; aí o resultado não vale como nota.
    pub fn run(&self, threads: usize, min_duration: Duration, cancel: &std::sync::atomic::AtomicBool) -> BenchResult {
        let threads = threads.max(1);
        let count = self.buckets.len();
        self.samples.store(0, Ordering::Relaxed);
        let start = Instant::now();
        let mut passes = 0;
        loop {
            self.pixels.lock().unwrap().fill(0);
            self.dirty.lock().unwrap().clear();
            self.done.store(0, Ordering::Relaxed);
            *self.active.lock().unwrap() = vec![-1; threads];
            self.pass.store(passes as i64 + 1, Ordering::Relaxed);
            let next = AtomicUsize::new(0);
            std::thread::scope(|s| {
                for t in 0..threads {
                    let next = &next;
                    std::thread::Builder::new()
                        .name(format!("RESET BENCH {t}"))
                        .spawn_scoped(s, move || loop {
                            if cancel.load(Ordering::Relaxed) {
                                break;
                            }
                            let b = next.fetch_add(1, Ordering::Relaxed);
                            if b >= count {
                                break;
                            }
                            self.active.lock().unwrap()[t] = b as i64;
                            if self.render_bucket(b, cancel) {
                                self.done.fetch_add(1, Ordering::Relaxed);
                            }
                            self.active.lock().unwrap()[t] = -1;
                        })
                        .expect("não deu para criar thread do benchmark");
                }
            });
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            passes += 1;
            if start.elapsed() >= min_duration {
                break;
            }
        }
        BenchResult::new(threads, start.elapsed().as_secs_f64(), self.samples(), passes)
    }

    /// Renderiza um bloco linha a linha, publicando cada linha pronta. false se foi cancelado no meio.
    fn render_bucket(&self, idx: usize, cancel: &std::sync::atomic::AtomicBool) -> bool {
        let b = self.buckets[idx];
        let mut row = vec![0u8; b.w];
        for y in b.y..b.y + b.h {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            for (i, x) in (b.x..b.x + b.w).enumerate() {
                row[i] = pixel(x, y, SPP);
            }
            self.pixels.lock().unwrap()[y * WIDTH + b.x..y * WIDTH + b.x + b.w].copy_from_slice(&row);
            self.dirty.lock().unwrap().push((idx, y, 1));
            self.samples.fetch_add((b.w * SPP) as u64, Ordering::Relaxed);
        }
        true
    }
}

/// Estado novo, pronto para ser dividido entre a thread que roda e a que mostra.
pub fn shared() -> Arc<RenderState> {
    Arc::new(RenderState::new())
}

/// Tom de cinza (0–255) de um pixel com `spp` amostras. Mesma semente e mesmas contas do C#.
pub fn pixel(x: usize, y: usize, spp: usize) -> u8 {
    let mut seed = ((y * WIDTH + x) as u32).wrapping_mul(9781).wrapping_add(0x9E37_79B9);
    let mut sum = 0f32;
    for _ in 0..spp {
        let u = (x as f32 + rnd(&mut seed)) / WIDTH as f32;
        let v = (y as f32 + rnd(&mut seed)) / HEIGHT as f32;
        sum += net_min(trace(camera_ray(u, v), &mut seed), 4.0); // limita "fireflies"
    }
    to_pixel(sum / spp as f32)
}

// ------------------------------------------------------------------ vetor (System.Numerics.Vector3)

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

const UNIT_X: V3 = v3(1.0, 0.0, 0.0);
const UNIT_Y: V3 = v3(0.0, 1.0, 0.0);

impl V3 {
    /// Mesma ordem do `dpps` que o .NET usa: (x·x + y·y) + z·z.
    #[inline(always)]
    fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline(always)]
    fn len_sq(self) -> f32 {
        self.dot(self)
    }
    #[inline(always)]
    fn normalize(self) -> V3 {
        self / self.len_sq().sqrt()
    }
    #[inline(always)]
    fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    /// Vector3.Reflect do .NET 8: v − (2·(v·n))·n.
    #[inline(always)]
    fn reflect(self, n: V3) -> V3 {
        self - n * (2.0 * self.dot(n))
    }
}

impl Add for V3 {
    type Output = V3;
    #[inline(always)]
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for V3 {
    type Output = V3;
    #[inline(always)]
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Neg for V3 {
    type Output = V3;
    #[inline(always)]
    fn neg(self) -> V3 {
        v3(-self.x, -self.y, -self.z)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    #[inline(always)]
    fn mul(self, k: f32) -> V3 {
        v3(self.x * k, self.y * k, self.z * k)
    }
}
impl Mul<V3> for f32 {
    type Output = V3;
    #[inline(always)]
    fn mul(self, v: V3) -> V3 {
        v * self
    }
}
impl Div<f32> for V3 {
    type Output = V3;
    #[inline(always)]
    fn div(self, k: f32) -> V3 {
        v3(self.x / k, self.y / k, self.z / k)
    }
}

/// MathF.Min/Max do .NET (propagam NaN e tratam -0 como menor que +0).
#[inline(always)]
fn net_min(a: f32, b: f32) -> f32 {
    if a != b {
        if a.is_nan() || a < b { a } else { b }
    } else if b.is_sign_negative() {
        b
    } else {
        a
    }
}
#[inline(always)]
fn net_max(a: f32, b: f32) -> f32 {
    if a != b {
        if a.is_nan() { a } else if a < b { b } else { a }
    } else if b.is_sign_negative() {
        a
    } else {
        b
    }
}

// ------------------------------------------------------------------ cena

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mat {
    Diffuse,
    Metal,
    Glass,
    Light,
}

#[derive(Debug, Clone, Copy)]
struct Sphere {
    c: V3,
    r: f32,
    m: Mat,
    albedo: f32,
    rough: f32,
}

const fn sphere(c: V3, r: f32, m: Mat, albedo: f32, rough: f32) -> Sphere {
    Sphere { c, r, m, albedo, rough }
}

const LAMP: Sphere = sphere(v3(-4.0, 7.0, -3.0), 2.2, Mat::Light, 0.0, 0.0);
const LAMP_POWER: f32 = 9.0;
const TAU: f32 = std::f32::consts::TAU;

// `black_box` força seno/cosseno/tangente a rodarem na libm (como o .NET faz), sem o compilador pré-calcular.
static SPHERES: LazyLock<Vec<Sphere>> = LazyLock::new(|| {
    let mut s = vec![
        sphere(v3(0.0, 1.1, 0.0), 1.1, Mat::Glass, 0.98, 0.0),
        sphere(v3(-2.5, 0.9, 0.9), 0.9, Mat::Metal, 0.92, 0.03),
        sphere(v3(2.4, 0.8, 0.7), 0.8, Mat::Diffuse, 0.85, 0.0),
        sphere(v3(1.0, 0.45, -1.9), 0.45, Mat::Metal, 0.85, 0.25),
        sphere(v3(-1.1, 0.35, -2.0), 0.35, Mat::Diffuse, 0.15, 0.0),
    ];
    // Anel de esferas pequenas alternando materiais.
    for i in 0..14 {
        let a = black_box(i as f32 / 14.0 * TAU);
        let m = match i % 3 {
            0 => Mat::Metal,
            1 => Mat::Diffuse,
            _ => Mat::Glass,
        };
        let albedo = if i % 2 == 0 { 0.9 } else { 0.3 };
        s.push(sphere(v3(a.cos() * 4.2, 0.28, a.sin() * 4.2 + 1.5), 0.28, m, albedo, 0.1));
    }
    s
});

struct Camera {
    origin: V3,
    lower: V3,
    horizontal: V3,
    vertical: V3,
}

static CAMERA: LazyLock<Camera> = LazyLock::new(|| {
    let origin = v3(0.0, 2.1, -7.2);
    let target = v3(0.0, 0.75, 0.0);
    let vfov = black_box(38f32) * std::f32::consts::PI / 180.0;
    let aspect = WIDTH as f32 / HEIGHT as f32;
    let hh = black_box(vfov / 2.0).tan();
    let hw = aspect * hh;
    let w = (origin - target).normalize();
    let u = UNIT_Y.cross(w).normalize();
    let v = w.cross(u);
    Camera {
        origin,
        lower: origin - hw * u + hh * v - w, // canto superior esquerdo (y cresce para baixo na imagem)
        horizontal: 2.0 * hw * u,
        vertical: -2.0 * hh * v,
    }
});

#[inline(always)]
fn camera_ray(s: f32, t: f32) -> (V3, V3) {
    let c = &*CAMERA;
    (c.origin, (c.lower + s * c.horizontal + t * c.vertical - c.origin).normalize())
}

// ------------------------------------------------------------------ path tracing

fn trace(ray: (V3, V3), seed: &mut u32) -> f32 {
    let spheres: &[Sphere] = &SPHERES;
    let mut radiance = 0f32;
    let mut throughput = 1f32;
    let mut specular_bounce = true; // câmera e reflexos especulares "veem" a lâmpada diretamente
    let (mut o, mut d) = ray;

    for depth in 0..MAX_DEPTH {
        let Some((t, idx, lamp)) = intersect(spheres, o, d) else {
            radiance += throughput * sky(d);
            break;
        };
        if lamp {
            if specular_bounce {
                radiance += throughput * LAMP_POWER;
            }
            break;
        }

        let p = o + d * t;
        let (n, mat, albedo, rough);
        if idx < 0 {
            // Chão: xadrez suave em tons de cinza
            n = UNIT_Y;
            mat = Mat::Diffuse;
            let check = (((p.x * 0.9).floor() as i32 + (p.z * 0.9).floor() as i32) & 1) == 0;
            albedo = if check { 0.62 } else { 0.38 };
            rough = 0.0;
        } else {
            let sp = &spheres[idx as usize];
            n = (p - sp.c) / sp.r;
            mat = sp.m;
            albedo = sp.albedo;
            rough = sp.rough;
        }

        match mat {
            Mat::Diffuse => {
                // Luz direta da lâmpada (amostragem por cone) + rebote difuso cosseno-ponderado.
                radiance += throughput * albedo * direct_light(spheres, p, n, seed);
                throughput *= albedo;
                d = cosine_hemisphere(n, seed);
                o = p + n * 1e-3;
                specular_bounce = false;
            }
            Mat::Metal => {
                let r = d.reflect(n) + rough * random_in_sphere(seed);
                if r.dot(n) <= 0.0 {
                    return radiance;
                }
                d = r.normalize();
                o = p + n * 1e-3;
                throughput *= albedo;
                specular_bounce = true;
            }
            Mat::Glass => {
                const IOR: f32 = 1.5;
                let outside = d.dot(n) < 0.0;
                let nn = if outside { n } else { -n };
                let eta = if outside { 1.0 / IOR } else { IOR };
                let cos = net_min(-d.dot(nn), 1.0);
                let sin2 = eta * eta * (1.0 - cos * cos);
                let mut r0 = (1.0 - IOR) / (1.0 + IOR);
                r0 *= r0;
                let fresnel = r0 + (1.0 - r0) * (1.0 - cos).powf(5.0);
                // `||` com curto-circuito: o RNG só avança se não houver reflexão total
                if sin2 > 1.0 || rnd(seed) < fresnel {
                    d = d.reflect(nn);
                    o = p + nn * 1e-3;
                } else {
                    d = (eta * d + (eta * cos - (1.0 - sin2).sqrt()) * nn).normalize();
                    o = p - nn * 1e-3;
                }
                throughput *= albedo;
                specular_bounce = true;
            }
            Mat::Light => {}
        }

        // Roleta russa depois de alguns rebotes.
        if depth >= 3 {
            let q = net_max(0.05, 1.0 - throughput);
            if rnd(seed) < q {
                break;
            }
            throughput /= 1.0 - q;
        }
    }
    radiance
}

/// Amostra direção dentro do cone que a lâmpada esférica ocupa e testa sombra.
#[inline(always)]
fn direct_light(spheres: &[Sphere], p: V3, n: V3, seed: &mut u32) -> f32 {
    let to_c = LAMP.c - p;
    let dist2 = to_c.len_sq();
    let cos_max = net_max(0.0, 1.0 - LAMP.r * LAMP.r / dist2).sqrt();
    let w = to_c / dist2.sqrt();

    let r1 = rnd(seed);
    let r2 = rnd(seed);
    let cos_t = 1.0 - r1 * (1.0 - cos_max);
    let sin_t = (1.0 - cos_t * cos_t).sqrt();
    let phi = TAU * r2;
    let (a, b) = basis(w);
    let dir = (a * (phi.cos() * sin_t) + b * (phi.sin() * sin_t) + w * cos_t).normalize();

    let cos_n = dir.dot(n);
    if cos_n <= 0.0 {
        return 0.0;
    }
    match intersect(spheres, p + n * 1e-3, dir) {
        Some((_, _, true)) => LAMP_POWER * cos_n * 2.0 * (1.0 - cos_max),
        _ => 0.0,
    }
}

/// (t, índice, é a lâmpada). Índice -1 = chão.
#[inline(always)]
fn intersect(spheres: &[Sphere], o: V3, d: V3) -> Option<(f32, i32, bool)> {
    let mut t_min = f32::MAX;
    let mut idx = -2;
    let mut lamp = false;
    // Chão (plano y = 0)
    if d.y < -1e-6 {
        let t = -o.y / d.y;
        if t > 1e-4 && t < 60.0 {
            t_min = t;
            idx = -1;
        }
    }
    for (i, s) in spheres.iter().enumerate() {
        if let Some(t) = hit_sphere(s, o, d, t_min) {
            t_min = t;
            idx = i as i32;
        }
    }
    if let Some(t) = hit_sphere(&LAMP, o, d, t_min) {
        t_min = t;
        lamp = true;
    }
    (idx != -2 || lamp).then_some((t_min, idx, lamp))
}

#[inline(always)]
fn hit_sphere(s: &Sphere, o: V3, d: V3, t_max: f32) -> Option<f32> {
    let oc = o - s.c;
    let b = oc.dot(d);
    let c = oc.len_sq() - s.r * s.r;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let mut t = -b - sq;
    if t < 1e-4 {
        t = -b + sq;
    }
    (t > 1e-4 && t < t_max).then_some(t)
}

#[inline(always)]
fn sky(d: V3) -> f32 {
    0.06 + 0.22 * net_max(0.0, d.y)
}

// ------------------------------------------------------------------ utilidades

/// xorshift32: rápido e determinístico por pixel.
#[inline(always)]
pub fn rnd(s: &mut u32) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    (*s >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline(always)]
fn random_in_sphere(s: &mut u32) -> V3 {
    loop {
        let x = rnd(s) * 2.0 - 1.0;
        let y = rnd(s) * 2.0 - 1.0;
        let z = rnd(s) * 2.0 - 1.0;
        let v = v3(x, y, z);
        if v.len_sq() < 1.0 {
            return v;
        }
    }
}

#[inline(always)]
fn cosine_hemisphere(n: V3, s: &mut u32) -> V3 {
    let r1 = rnd(s);
    let r2 = rnd(s);
    let r = r1.sqrt();
    let phi = TAU * r2;
    let (a, b) = basis(n);
    (a * (r * phi.cos()) + b * (r * phi.sin()) + n * (1.0 - r1).sqrt()).normalize()
}

#[inline(always)]
fn basis(n: V3) -> (V3, V3) {
    let a = (if n.x.abs() > 0.9 { UNIT_Y } else { UNIT_X }).cross(n).normalize();
    (a, n.cross(a))
}

/// Tone mapping (ACES aproximado) + gama 2.2 → tom de cinza.
pub fn to_pixel(v: f32) -> u8 {
    let v = v * 1.1;
    let v = v * (2.51 * v + 0.03) / (v * (2.43 * v + 0.59) + 0.14);
    let c = v.clamp(0.0, 1.0); // Math.Clamp (NaN passa direto nos dois)
    (c.powf(1.0 / 2.2) * 255.0 + 0.5) as i32 as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    // Valores de referência tirados do BenchRenderer.cs original rodando no .NET 8 (Linux x64).

    #[test]
    fn rng_igual_ao_csharp() {
        let mut s = 0x9E37_79B9u32;
        let want = [(1359758873u32, 0x3EA2188Cu32), (3761132862, 0x3F602E55), (2075758394, 0x3EF7731E), (25405621, 0x3BC1D400), (3862129951, 0x3F66336D)];
        for (state, bits) in want {
            let r = rnd(&mut s);
            assert_eq!((s, r.to_bits()), (state, bits));
        }
    }

    #[test]
    fn camera_e_tone_mapping() {
        let (_, d) = camera_ray(0.5, 0.5);
        assert_eq!([d.x.to_bits(), d.y.to_bits(), d.z.to_bits()], [0x00000000, 0xBE3CB62A, 0x3F7B9D83]);
        let (o, d) = camera_ray(0.1, 0.9);
        assert_eq!([d.x.to_bits(), d.y.to_bits(), d.z.to_bits()], [0x3EDA9736, 0xBECB1C94, 0x3F5007F7]);
        assert_eq!(trace((o, d), &mut 12345).to_bits(), 0x3E67AD5A);
        assert_eq!([to_pixel(0.5), to_pixel(1.7), to_pixel(0.02)], [209, 244, 34]);
    }

    #[test]
    fn blocos_do_centro_para_fora() {
        let b = buckets();
        assert_eq!(b.len(), 510);
        let first: Vec<(usize, usize)> = b.iter().take(8).map(|b| (b.x, b.y)).collect();
        assert_eq!(first, [(448, 256), (480, 256), (416, 256), (512, 256), (448, 224), (480, 224), (448, 288), (480, 288)]);
        assert_eq!((b[509].x, b[509].y), (928, 512));
        assert!(b.iter().all(|b| b.w == 32 && (b.h == 32 || b.h == 28)));
    }

    #[test]
    fn pixels_iguais_ao_csharp() {
        let want = [((480, 270), 93), ((0, 0), 97), ((959, 539), 196), ((100, 400), 147), ((700, 150), 75), ((300, 300), 131), ((520, 330), 152), ((250, 420), 89), ((640, 200), 132), ((830, 470), 195)];
        for ((x, y), g) in want {
            assert_eq!(pixel(x, y, SPP), g, "pixel ({x}, {y})");
        }
    }

    /// A imagem inteira com 1 e 4 amostras por pixel (rápido) bate byte a byte com a do C#.
    #[test]
    fn imagem_inteira_igual_ao_csharp() {
        for (spp, want) in [(1, 0xB8072C73u32), (4, 0x29EB21D6)] {
            let mut h = 2166136261u32;
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    h ^= pixel(x, y, spp) as u32;
                    h = h.wrapping_mul(16777619);
                }
            }
            assert_eq!(h, want, "spp={spp}");
        }
    }

    /// Medição curta (5 s multi + 5 s single, cancelada no meio da passada): só uma ideia da nota.
    /// `cargo test --release -p reset-core nota_curta -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn nota_curta() {
        for threads in [std::thread::available_parallelism().unwrap().get(), 1] {
            let st = Arc::new(RenderState::new());
            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (s2, c2) = (st.clone(), cancel.clone());
            let h = std::thread::spawn(move || s2.run(threads, Duration::ZERO, &c2));
            std::thread::sleep(Duration::from_secs(5));
            let samples = st.samples();
            cancel.store(true, Ordering::Relaxed);
            h.join().unwrap();
            println!("{threads} threads: {:.0} pts (parcial, {} blocos)", samples as f64 / 5.0 / 10_000.0, st.buckets_done());
        }
    }

    #[test]
    fn nota_e_cancelamento() {
        assert_eq!(BenchResult::new(16, 10.0, 450_000_000, 1).score, 4500.0);
        let st = RenderState::new();
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let r = st.run(4, Duration::ZERO, &cancel);
        assert_eq!((r.passes, r.samples), (0, 0));
    }
}
