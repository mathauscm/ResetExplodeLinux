//! Testes do que pesa em jogos fora da CPU/GPU, portados do `MemoryBench.cs`: latência e banda da RAM, e
//! velocidade de "carregamento de mapa" (um pacote grande + muitos arquivos pequenos) lida do disco sem cache.
//!
//! No Windows a leitura sem cache usava FILE_FLAG_NO_BUFFERING. Aqui os arquivos são gravados, sincronizados
//! e tirados do cache do kernel (`posix_fadvise(DONTNEED)`) antes de cada leitura medida. Não há antivírus
//! para a passada prévia que o Windows precisava. A pasta fica em ~/.cache (o /tmp costuma ser tmpfs, na RAM).

use serde::Serialize;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const LATENCY_BYTES: usize = 256 << 20; // 256 MB: bem maior que qualquer cache L3
const BANDWIDTH_BYTES: usize = 1 << 30; // 1 GB

#[derive(Debug, Clone, Copy)]
pub struct LoadSizes {
    /// "Pacote" do mapa.
    pub pack: u64,
    pub assets: usize,
    pub asset_bytes: usize,
}

/// Mesmo tamanho do Windows: pacote de 512 MB + mil arquivos de 64 KB.
pub const LOAD_DEFAULT: LoadSizes = LoadSizes { pack: 512 << 20, assets: 1000, asset_bytes: 64 << 10 };

#[derive(Debug, Clone, Serialize)]
pub struct LoadResult {
    pub seq_read_mbs: f64,
    pub small_files_mbs: f64,
    pub files_per_sec: f64,
    pub map_load_seconds: f64,
    /// Unidade onde ficaram os arquivos de teste (ponto de montagem).
    pub drive: String,
}

/// Gerador simples (xorshift64*) para ordens e dados aleatórios determinísticos.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Inteiro em [0, n).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        ((self.next() >> 32) * n) >> 32
    }
    pub(crate) fn fill(&mut self, buf: &mut [u8]) {
        for c in buf.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            c.copy_from_slice(&v[..c.len()]);
        }
    }
}

// ------------------------------------------------------------------ RAM

/// Latência: percorre uma lista encadeada em ordem aleatória (permutação de Sattolo = um único ciclo).
/// Cada leitura depende da anterior, então o processador não consegue adiantar: mede o tempo de ida à RAM.
pub fn latency_ns(cancel: &AtomicBool) -> f64 {
    latency_ns_with(LATENCY_BYTES, 20_000_000, cancel)
}

fn latency_ns_with(bytes: usize, steps: usize, cancel: &AtomicBool) -> f64 {
    let n = bytes / 8;
    let next = sattolo_chain(n, 42);
    let mut p = 0usize;
    for _ in 0..200_000.min(steps) {
        p = next[p] as usize; // aquece TLB/páginas
    }
    let sw = Instant::now();
    let mut done = 0;
    for i in 0..steps {
        p = next[p] as usize;
        done = i + 1;
        if i & 0xFFFFF == 0 && cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    std::hint::black_box(p);
    sw.elapsed().as_secs_f64() * 1e9 / done.max(1) as f64
}

/// Saltos de 64 bytes (linhas de cache diferentes) num único ciclo aleatório que passa por todas as linhas.
fn sattolo_chain(n: usize, seed: u64) -> Vec<u64> {
    let lines = n / 8;
    let mut order: Vec<u64> = (0..lines as u64).collect();
    let mut rng = Rng::new(seed);
    for i in (1..lines).rev() {
        let j = rng.below(i as u64) as usize;
        order.swap(i, j);
    }
    let mut next = vec![0u64; n];
    for i in 0..lines {
        next[order[i] as usize * 8] = order[(i + 1) % lines] * 8;
    }
    next
}

/// Banda de leitura (soma) e de cópia com todas as threads, em GB/s (melhor de 3).
pub fn bandwidth(cancel: &AtomicBool) -> (f64, f64) {
    bandwidth_with(BANDWIDTH_BYTES, cancel)
}

fn bandwidth_with(bytes: usize, cancel: &AtomicBool) -> (f64, f64) {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let n = bytes / 8;
    let per = n / threads;
    let mut src = vec![0u64; n];
    let mut dst = vec![0u64; n];
    std::thread::scope(|s| {
        for (t, c) in src.chunks_mut(per.max(1)).enumerate() {
            s.spawn(move || c.fill(t as u64 + 1));
        }
    });

    let (mut best_read, mut best_copy) = (0f64, 0f64);
    for _ in 0..3 {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let sw = Instant::now();
        let total: u64 = std::thread::scope(|s| {
            let hs: Vec<_> = src
                .chunks(per.max(1))
                .map(|c| {
                    s.spawn(move || {
                        let (mut s0, mut s1, mut s2, mut s3) = (0u64, 0u64, 0u64, 0u64);
                        for q in c.as_chunks::<4>().0 {
                            s0 = s0.wrapping_add(q[0]);
                            s1 = s1.wrapping_add(q[1]);
                            s2 = s2.wrapping_add(q[2]);
                            s3 = s3.wrapping_add(q[3]);
                        }
                        s0.wrapping_add(s1).wrapping_add(s2).wrapping_add(s3)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or(0)).fold(0, u64::wrapping_add)
        });
        std::hint::black_box(total);
        best_read = best_read.max(bytes as f64 / sw.elapsed().as_secs_f64() / 1e9);

        let sw = Instant::now();
        std::thread::scope(|s| {
            for (d, c) in dst.chunks_mut(per.max(1)).zip(src.chunks(per.max(1))) {
                s.spawn(move || d.copy_from_slice(c));
            }
        });
        std::hint::black_box(&dst);
        // Cópia lê e escreve: conta os dois sentidos, como os benchmarks de memória fazem.
        best_copy = best_copy.max(2.0 * bytes as f64 / sw.elapsed().as_secs_f64() / 1e9);
    }
    (best_read, best_copy)
}

// ------------------------------------------------------------------ carregamento (disco)

/// Pasta dos arquivos de teste: ~/.cache/reset-explode-bench (fica no disco do usuário, não na RAM).
pub fn test_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("reset-explode-bench")
}

/// Ponto de montagem que contém `path` (o mais longo de /proc/self/mounts que é prefixo).
pub fn mount_of(path: &Path) -> String {
    let text = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let p = path.to_string_lossy();
    text.lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|m| m.replace("\\040", " "))
        .filter(|m| m == "/" || p == m.as_str() || p.starts_with(&format!("{m}/")))
        .max_by_key(|m| m.len())
        .unwrap_or_else(|| "/".into())
}

/// Tira um arquivo do cache do kernel (precisa estar gravado no disco: chame depois de `sync_data`).
pub(crate) fn drop_cache(f: &File) {
    unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

/// Cria os arquivos de teste, lê sem cache e apaga tudo no final. `progress` recebe (fase, fração).
pub fn loading(dir: &Path, sizes: LoadSizes, progress: &dyn Fn(&str, f64), cancel: &AtomicBool) -> Result<LoadResult, String> {
    let r = loading_inner(dir, sizes, progress, cancel);
    let _ = fs::remove_dir_all(dir);
    r
}

fn loading_inner(dir: &Path, sz: LoadSizes, progress: &dyn Fn(&str, f64), cancel: &AtomicBool) -> Result<LoadResult, String> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let err = |e: std::io::Error| format!("arquivos de teste em {}: {e}", dir.display());
    fs::create_dir_all(dir).map_err(err)?;
    let drive = mount_of(dir);
    let pack = dir.join("mapa.pak");
    let total = sz.pack as f64 + (sz.assets * sz.asset_bytes) as f64;

    // 1) Preparação (escrita) — não entra na nota.
    let mut block = vec![0u8; 4 << 20];
    Rng::new(7).fill(&mut block); // dados incompressíveis
    {
        let mut f = File::create(&pack).map_err(err)?;
        let mut w = 0u64;
        while w < sz.pack && !cancelled() {
            let n = (sz.pack - w).min(block.len() as u64) as usize;
            f.write_all(&block[..n]).map_err(err)?;
            w += n as u64;
            progress("Preparando arquivos de teste", w as f64 / total);
        }
        f.sync_data().map_err(err)?;
    }
    for i in 0..sz.assets {
        if cancelled() {
            break;
        }
        let mut f = File::create(dir.join(format!("asset{i:04}.dat"))).map_err(err)?;
        f.write_all(&block[(i * 4096) % (block.len() - sz.asset_bytes).max(1)..][..sz.asset_bytes]).map_err(err)?;
        if i % 50 == 0 {
            progress("Preparando arquivos de teste", (sz.pack as f64 + (i * sz.asset_bytes) as f64) / total);
        }
    }
    if cancelled() {
        return Err("cancelado".into());
    }
    // grava tudo de uma vez (mais rápido que fsync por arquivo) e tira do cache
    unsafe { libc::sync() };
    progress("Tirando os arquivos do cache", 0.0);
    for i in 0..sz.assets {
        if let Ok(f) = File::open(dir.join(format!("asset{i:04}.dat"))) {
            drop_cache(&f);
        }
    }

    // 2) Leitura sequencial do pacote, sem cache.
    let mut f = File::open(&pack).map_err(err)?;
    drop_cache(&f);
    let sw = Instant::now();
    let mut off = 0u64;
    while off < sz.pack && !cancelled() {
        let n = f.read(&mut block).map_err(err)?;
        if n == 0 {
            break;
        }
        off += n as u64;
        progress("Lendo pacote do mapa", off as f64 / sz.pack as f64);
    }
    let seq_secs = sw.elapsed().as_secs_f64();
    let seq_mbs = sz.pack as f64 / 1_048_576.0 / seq_secs;

    // 3) Muitos arquivos pequenos (texturas, sons, scripts): abrir + ler + fechar, com 8 threads, como as engines.
    progress("Lendo assets pequenos (8 threads)", 0.5);
    let sw = Instant::now();
    read_assets(dir, sz, 8, cancel);
    let secs = sw.elapsed().as_secs_f64();
    if cancelled() {
        return Err("cancelado".into());
    }
    let mb = (sz.assets * sz.asset_bytes) as f64 / 1_048_576.0;
    Ok(LoadResult {
        seq_read_mbs: seq_mbs,
        small_files_mbs: mb / secs,
        files_per_sec: sz.assets as f64 / secs,
        map_load_seconds: seq_secs + secs,
        drive,
    })
}

fn read_assets(dir: &Path, sz: LoadSizes, threads: usize, cancel: &AtomicBool) {
    std::thread::scope(|s| {
        for t in 0..threads {
            s.spawn(move || {
                let mut buf = vec![0u8; sz.asset_bytes];
                let mut i = t;
                while i < sz.assets && !cancel.load(Ordering::Relaxed) {
                    if let Ok(mut f) = File::open(dir.join(format!("asset{i:04}.dat"))) {
                        let _ = f.read_exact(&mut buf);
                    }
                    i += threads;
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadeia_passa_por_todas_as_linhas() {
        let n = 8 * 1000;
        let next = sattolo_chain(n, 42);
        let mut p = 0usize;
        let mut seen = vec![false; n / 8];
        for _ in 0..n / 8 {
            assert!(!seen[p / 8], "ciclo curto");
            seen[p / 8] = true;
            p = next[p] as usize;
        }
        assert_eq!(p, 0);
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn ram_rapido() {
        let c = AtomicBool::new(false);
        let ns = latency_ns_with(4 << 20, 200_000, &c);
        assert!(ns > 0.0 && ns < 1000.0);
        let (r, w) = bandwidth_with(16 << 20, &c);
        assert!(r > 0.0 && w > 0.0);
    }

    #[test]
    fn carregamento_pequeno_e_apaga_tudo() {
        let dir = std::env::temp_dir().join(format!("reset-explode-load-test-{}", std::process::id()));
        let c = AtomicBool::new(false);
        let r = loading(&dir, LoadSizes { pack: 4 << 20, assets: 20, asset_bytes: 64 << 10 }, &|_, _| {}, &c).unwrap();
        assert!(r.seq_read_mbs > 0.0 && r.files_per_sec > 0.0 && r.map_load_seconds > 0.0);
        assert!(!dir.exists());
    }
}
