//! Discos físicos e unidades montadas (equivalente ao `DriveMap.cs`): /sys/block para os discos,
//! /proc/self/mountinfo + statvfs para o quanto cada unidade está cheia.

use crate::util::{read_trim, read_u64};
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    /// "nvme0n1", "sda"
    pub name: String,
    pub model: String,
    pub serial: Option<String>,
    pub size: u64,
    /// "NVMe", "SSD", "HD", "USB"
    pub kind: String,
    pub removable: bool,
    pub volumes: Vec<Volume>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub device: String,
    pub mount: String,
    pub fs: String,
    pub label: Option<String>,
    pub total: u64,
    pub free: u64,
}

pub fn list() -> Vec<Disk> {
    let labels = labels();
    let mounts = mounts();
    let Ok(dir) = fs::read_dir("/sys/block") else { return Vec::new() };
    let mut disks: Vec<Disk> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if ["loop", "ram", "zram", "dm-", "md", "sr", "fd"].iter().any(|p| name.starts_with(p)) {
                return None;
            }
            let p = e.path();
            let size = read_u64(p.join("size"))? * 512;
            if size == 0 {
                return None;
            }
            let removable = read_u64(p.join("removable")) == Some(1);
            let rotational = read_u64(p.join("queue/rotational")) == Some(1);
            let usb = fs::canonicalize(&p).map(|c| c.to_string_lossy().contains("/usb")).unwrap_or(false);
            let kind = if usb {
                "USB"
            } else if name.starts_with("nvme") {
                "NVMe"
            } else if rotational {
                "HD"
            } else {
                "SSD"
            };
            let model = read_trim(p.join("device/model"))
                .or_else(|| read_trim(p.join("device/name")))
                .unwrap_or_else(|| name.clone());

            // partições ficam em /sys/block/<disco>/<disco>pN; o disco inteiro também pode estar montado (pendrive sem partição)
            let mut parts: Vec<String> = fs::read_dir(&p)
                .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&name)).collect())
                .unwrap_or_default();
            parts.sort();
            parts.insert(0, name.clone());
            let mut volumes = Vec::new();
            for part in parts {
                for (mount, fs) in mounts.get(&format!("/dev/{part}")).into_iter().flatten() {
                    let (total, free) = usage(mount);
                    volumes.push(Volume {
                        device: format!("/dev/{part}"),
                        mount: mount.clone(),
                        fs: fs.clone(),
                        label: labels.get(&part).cloned(),
                        total,
                        free,
                    });
                }
            }
            // btrfs com subvolumes monta a mesma partição várias vezes: mostra só a raiz mais curta
            volumes.sort_by_key(|v| v.mount.len());
            let mut seen = std::collections::HashSet::new();
            volumes.retain(|v| v.fs != "btrfs" || seen.insert(v.device.clone()));

            Some(Disk {
                serial: read_trim(p.join("device/serial")),
                name,
                model,
                size,
                kind: kind.into(),
                removable: removable || usb,
                volumes,
            })
        })
        .collect();
    disks.sort_by(|a, b| a.removable.cmp(&b.removable).then(a.name.cmp(&b.name)));
    disks
}

/// /dev/xxx -> [(ponto de montagem, sistema de arquivos)]
fn mounts() -> HashMap<String, Vec<(String, String)>> {
    let mut m: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for l in fs::read_to_string("/proc/self/mounts").unwrap_or_default().lines() {
        let mut it = l.split_whitespace();
        let (Some(dev), Some(mnt), Some(fs)) = (it.next(), it.next(), it.next()) else { continue };
        if !dev.starts_with("/dev/") {
            continue;
        }
        let dev = fs::canonicalize(dev).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| dev.to_string());
        m.entry(dev).or_default().push((unescape(mnt), fs.to_string()));
    }
    m
}

/// "sda1" -> "MEU PENDRIVE", a partir de /dev/disk/by-label.
fn labels() -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(dir) = fs::read_dir("/dev/disk/by-label") {
        for e in dir.flatten() {
            if let Ok(t) = fs::canonicalize(e.path()) {
                if let Some(n) = t.file_name() {
                    m.insert(n.to_string_lossy().into_owned(), unescape(&e.file_name().to_string_lossy()));
                }
            }
        }
    }
    m
}

/// /proc/mounts e by-label escapam espaço e afins como \040 / \x20.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push(u8::from_str_radix(&s[i + 1..i + 4], 8).unwrap_or(b'?'));
            i += 4;
        } else if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// (total, livre para o usuário) em bytes.
fn usage(mount: &str) -> (u64, u64) {
    let Ok(c) = CString::new(Path::new(mount).as_os_str().as_encoded_bytes()) else { return (0, 0) };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return (0, 0);
    }
    let f = st.f_frsize as u64;
    (st.f_blocks as u64 * f, st.f_bavail as u64 * f)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unescape() {
        assert_eq!(super::unescape(r"/run/media/u/MEU\040PEN"), "/run/media/u/MEU PEN");
        assert_eq!(super::unescape(r"MEU\x20PEN"), "MEU PEN");
    }
}
