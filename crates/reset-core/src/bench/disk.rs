//! Benchmark de armazenamento no estilo CrystalDiskMark, portado do `DiskBench.cs`: leitura e escrita
//! sequenciais (blocos de 1 MB) e acesso aleatório de 4 KB (1 e 8 requisições em paralelo).
//!
//! No Windows: FILE_FLAG_NO_BUFFERING + WRITE_THROUGH. Aqui: O_DIRECT + O_DSYNC, com buffers alinhados em 4 KB.
//! No btrfs o arquivo é marcado como NOCOW antes de gravar (sem compressão nem cópia na escrita), senão o
//! kernel pode cair para escrita com cache. Se o sistema de arquivos não aceitar O_DIRECT, o teste roda com
//! cache e tira o arquivo do cache antes das leituras (o resultado avisa). O arquivo é apagado no final.

use super::memory::{drop_cache, Rng};
use crate::disks;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

const BLOCK: usize = 1 << 20; // 1 MB (sequencial)
const SMALL: usize = 4096; // 4 KB (aleatório)
/// Espaço livre mínimo para a unidade aparecer na lista.
const MIN_FREE: u64 = 1536 << 20;

#[derive(Debug, Clone, Serialize)]
pub struct Drive {
    /// Ponto de montagem ("/", "/mnt/jogos").
    pub mount: String,
    pub text: String,
    /// Pasta onde o arquivo de teste é criado.
    pub dir: PathBuf,
    pub free: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskResult {
    pub drive: String,
    pub file_mb: u64,
    pub seq_read_mbs: f64,
    pub seq_write_mbs: f64,
    pub read_iops_q1: f64,
    pub write_iops_q1: f64,
    pub read_latency_us: f64,
    pub write_latency_us: f64,
    pub read_iops_q8: f64,
    /// false = o sistema de arquivos não aceitou O_DIRECT e o teste passou pelo cache.
    pub direct: bool,
    pub score: f64,
}

/// Nota só para a barra do histórico: sequencial pesa em cópia/carregamento, 4K em abrir programas e sistema.
fn score(r: &DiskResult) -> f64 {
    (r.seq_read_mbs + r.seq_write_mbs) / 2.0 + (r.read_iops_q1 + r.write_iops_q1) / 200.0 + r.read_iops_q8 / 2000.0
}

/// Unidades que podem ser testadas: montadas em disco de verdade, com pasta gravável e 1,5 GB livres.
/// A unidade onde está a pasta pessoal usa ~/.cache; as outras, uma pasta oculta na raiz da unidade.
pub fn drives() -> Vec<Drive> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    let home_mount = super::memory::mount_of(&home);
    let mut out: Vec<Drive> = Vec::new();
    let mut devices: Vec<String> = Vec::new();
    let mut vols: Vec<(disks::Disk, disks::Volume)> =
        disks::list().into_iter().flat_map(|d| d.volumes.clone().into_iter().map(move |v| (d.clone(), v))).collect();
    // a unidade da pasta pessoal primeiro: com subvolumes (btrfs), "/" e "/home" são o mesmo disco
    vols.sort_by_key(|(_, v)| v.mount != home_mount);
    for (d, v) in vols {
        if v.free < MIN_FREE || v.mount.starts_with("/boot") || v.mount.starts_with("/efi") || devices.contains(&v.device) {
            continue;
        }
        let dir = if v.mount == home_mount { super::memory::test_dir() } else { Path::new(&v.mount).join(".reset-explode-bench") };
        let probe = if dir.exists() { dir.clone() } else { PathBuf::from(&v.mount) };
        if !writable(&probe) {
            continue;
        }
        devices.push(v.device.clone());
        let name = v.label.clone().unwrap_or_else(|| if v.mount == "/" { "Sistema".into() } else { v.mount.rsplit('/').next().unwrap_or("").to_string() });
        let name = if v.mount == home_mount && v.mount != "/" { format!("{name} (pasta pessoal)") } else { name };
        out.push(Drive { text: format!("{name} · {} · {}", d.kind, d.model), mount: v.mount, dir, free: v.free });
    }
    out
}

fn writable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.to_string_lossy().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Buffer alinhado em 4 KB (exigência do O_DIRECT).
struct Aligned {
    ptr: *mut u8,
    len: usize,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(len, 4096).expect("layout");
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "sem memória para o buffer do teste de disco");
        Self { ptr, len }
    }
    fn slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Aligned {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr, std::alloc::Layout::from_size_align(self.len, 4096).unwrap()) };
    }
}

/// btrfs: NOCOW num arquivo ainda vazio (desliga compressão e checksums; o O_DIRECT passa a ser direto mesmo).
fn set_nocow(f: &File) {
    const FS_IOC_GETFLAGS: libc::c_ulong = 0x8008_6601;
    const FS_IOC_SETFLAGS: libc::c_ulong = 0x4008_6602;
    const FS_NOCOW_FL: libc::c_int = 0x0080_0000;
    use std::os::fd::AsRawFd;
    let mut flags: libc::c_int = 0;
    unsafe {
        if libc::ioctl(f.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags) == 0 {
            flags |= FS_NOCOW_FL;
            libc::ioctl(f.as_raw_fd(), FS_IOC_SETFLAGS, &flags);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiskOptions {
    pub file_mb: u64,
    /// Duração de cada teste aleatório (4 s no app).
    pub random_secs: f64,
}

/// Roda o teste na pasta `dir` (de `drives()`); `progress` recebe (fase, fração). Apaga o arquivo no final.
pub fn run(dir: &Path, drive: &str, opt: DiskOptions, progress: &dyn Fn(&str, f64), cancel: &AtomicBool) -> Result<DiskResult, String> {
    fs::create_dir_all(dir).map_err(|e| format!("não deu para criar {}: {e}", dir.display()))?;
    let path = dir.join("bench.tmp");
    let r = run_inner(&path, drive, opt, progress, cancel);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(dir);
    r
}

fn open(path: &Path, direct: bool) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(true);
    o.custom_flags(if direct { libc::O_DIRECT | libc::O_DSYNC } else { libc::O_DSYNC });
    o.open(path)
}

fn run_inner(path: &Path, drive: &str, opt: DiskOptions, progress: &dyn Fn(&str, f64), cancel: &AtomicBool) -> Result<DiskResult, String> {
    let file_bytes = opt.file_mb << 20;
    let free = statvfs_free(path.parent().unwrap_or(Path::new("/")));
    if free < file_bytes + (512 << 20) {
        return Err(format!("pouco espaço livre em {drive} (precisa de {} MB).", opt.file_mb + 512));
    }
    let cancelled = || cancel.load(Ordering::Relaxed);
    let io = |e: std::io::Error| format!("erro de leitura/escrita: {e}");

    // cria vazio, marca NOCOW e reabre com O_DIRECT (se o sistema de arquivos aceitar)
    drop(File::create(path).map_err(io).inspect(set_nocow)?);
    let (f, direct) = match open(path, true) {
        Ok(f) => (f, true),
        Err(_) => (open(path, false).map_err(io)?, false),
    };

    let mut seq = Aligned::new(BLOCK);
    Rng::new(42).fill(seq.slice()); // dados incompressíveis (não deixam o controlador "trapacear")

    // ---------- Sequencial: escrita ----------
    let sw = Instant::now();
    let mut off = 0;
    while off < file_bytes {
        if cancelled() {
            return Err("cancelado".into());
        }
        f.write_all_at(seq.slice(), off).map_err(io)?;
        off += BLOCK as u64;
        progress("Escrita sequencial (1 MB)", 0.25 * off as f64 / file_bytes as f64);
    }
    let seq_write = opt.file_mb as f64 / sw.elapsed().as_secs_f64();
    if !direct {
        f.sync_data().map_err(io)?;
        drop_cache(&f);
    }

    // ---------- Sequencial: leitura ----------
    let sw = Instant::now();
    let mut off = 0;
    while off < file_bytes {
        if cancelled() {
            return Err("cancelado".into());
        }
        f.read_exact_at(seq.slice(), off).map_err(io)?;
        off += BLOCK as u64;
        progress("Leitura sequencial (1 MB)", 0.25 + 0.25 * off as f64 / file_bytes as f64);
    }
    let seq_read = opt.file_mb as f64 / sw.elapsed().as_secs_f64();

    let slots = file_bytes / SMALL as u64;
    if !direct {
        drop_cache(&f);
    }
    let (read_iops, read_lat) = random_loop(&f, slots, false, opt.random_secs, progress, "Leitura 4 KB aleatória (1 por vez)", 0.50, cancel)?;
    let (write_iops, write_lat) = random_loop(&f, slots, true, opt.random_secs, progress, "Escrita 4 KB aleatória (1 por vez)", 0.65, cancel)?;
    if !direct {
        f.sync_data().map_err(io)?;
        drop_cache(&f);
    }
    progress("Leitura 4 KB aleatória (8 em paralelo)", 0.80);
    let q8 = parallel_read(&f, slots, 8, opt.random_secs, progress, cancel)?;
    progress("Concluído", 1.0);

    let mut r = DiskResult {
        drive: drive.to_string(),
        file_mb: opt.file_mb,
        seq_read_mbs: seq_read,
        seq_write_mbs: seq_write,
        read_iops_q1: read_iops,
        write_iops_q1: write_iops,
        read_latency_us: read_lat,
        write_latency_us: write_lat,
        read_iops_q8: q8,
        direct,
        score: 0.0,
    };
    r.score = score(&r);
    Ok(r)
}

#[allow(clippy::too_many_arguments)]
fn random_loop(f: &File, slots: u64, write: bool, secs: f64, progress: &dyn Fn(&str, f64), phase: &str, start: f64, cancel: &AtomicBool) -> Result<(f64, f64), String> {
    let mut buf = Aligned::new(SMALL);
    let mut rng = Rng::new(if write { 7 } else { 3 });
    let mut ops = 0u64;
    let sw = Instant::now();
    while sw.elapsed().as_secs_f64() < secs {
        // checa o cancelamento a cada 64 operações (barato) e atualiza a barra
        for _ in 0..64 {
            let off = rng.below(slots) * SMALL as u64;
            if write { f.write_all_at(buf.slice(), off) } else { f.read_exact_at(buf.slice(), off) }.map_err(|e| format!("erro de leitura/escrita: {e}"))?;
            ops += 1;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelado".into());
        }
        progress(phase, start + 0.15 * (sw.elapsed().as_secs_f64() / secs).min(1.0));
    }
    let s = sw.elapsed().as_secs_f64();
    Ok((ops as f64 / s, s / ops as f64 * 1e6))
}

fn parallel_read(f: &File, slots: u64, threads: usize, secs: f64, progress: &dyn Fn(&str, f64), cancel: &AtomicBool) -> Result<f64, String> {
    let total = AtomicU64::new(0);
    let sw = Instant::now();
    std::thread::scope(|s| {
        for t in 0..threads {
            let total = &total;
            s.spawn(move || {
                let mut buf = Aligned::new(SMALL);
                let mut rng = Rng::new(100 + t as u64);
                let mut local = 0;
                while sw.elapsed().as_secs_f64() < secs && !cancel.load(Ordering::Relaxed) {
                    for _ in 0..32 {
                        if f.read_exact_at(buf.slice(), rng.below(slots) * SMALL as u64).is_err() {
                            return;
                        }
                        local += 1;
                    }
                }
                total.fetch_add(local, Ordering::Relaxed);
            });
        }
        while sw.elapsed().as_secs_f64() < secs && !cancel.load(Ordering::Relaxed) {
            progress("Leitura 4 KB aleatória (8 em paralelo)", 0.80 + 0.19 * (sw.elapsed().as_secs_f64() / secs).min(1.0));
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelado".into());
    }
    Ok(total.load(Ordering::Relaxed) as f64 / sw.elapsed().as_secs_f64())
}

fn statvfs_free(p: &Path) -> u64 {
    let Ok(c) = std::ffi::CString::new(p.to_string_lossy().as_bytes()) else { return 0 };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return 0;
    }
    s.f_bavail as u64 * s.f_frsize as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arquivo pequeno (8 MB) e testes aleatórios curtos, numa pasta temporária que some no final.
    #[test]
    fn disco_pequeno_e_apaga_o_arquivo() {
        let dir = std::env::temp_dir().join(format!("reset-explode-disk-test-{}", std::process::id()));
        let c = AtomicBool::new(false);
        let r = run(&dir, "teste", DiskOptions { file_mb: 8, random_secs: 0.15 }, &|_, _| {}, &c).unwrap();
        assert!(r.seq_read_mbs > 0.0 && r.seq_write_mbs > 0.0 && r.read_iops_q1 > 0.0 && r.write_iops_q1 > 0.0 && r.read_iops_q8 > 0.0);
        assert!(r.score > 0.0);
        assert!(!dir.exists());
    }

    #[test]
    fn cancelado_apaga_o_arquivo() {
        let dir = std::env::temp_dir().join(format!("reset-explode-disk-cancel-{}", std::process::id()));
        let c = AtomicBool::new(true);
        assert!(run(&dir, "teste", DiskOptions { file_mb: 8, random_secs: 0.1 }, &|_, _| {}, &c).is_err());
        assert!(!dir.exists());
    }
}
