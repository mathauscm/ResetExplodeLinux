use std::fs;
use std::path::Path;

/// Lê um arquivo de texto do /sys ou /proc e tira espaços/quebra de linha. None se não existe ou está vazio.
pub fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

pub fn read_u64(path: impl AsRef<Path>) -> Option<u64> {
    read_trim(path)?.parse().ok()
}

pub fn read_i64(path: impl AsRef<Path>) -> Option<i64> {
    read_trim(path)?.parse().ok()
}

/// Lê um número hexadecimal como "0x1462" ou "1462".
pub fn read_hex(path: impl AsRef<Path>) -> Option<u32> {
    let s = read_trim(path)?;
    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// Valores de placeholder que fabricantes deixam no DMI ("To Be Filled By O.E.M." etc.).
pub fn dmi_clean(s: Option<String>) -> Option<String> {
    let s = s?;
    let l = s.to_lowercase();
    let junk = ["to be filled", "default string", "not specified", "system product name", "system manufacturer", "o.e.m."];
    (!junk.iter().any(|j| l.contains(j))).then_some(s)
}

/// Bytes em texto curto: 512 MB, 31,2 GB, 1,8 TB (vírgula decimal, como no app do Windows).
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let s = if i == 0 || v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} {}", s.replace('.', ","), U[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1,5 KB");
        assert_eq!(human_bytes(32 * 1024 * 1024 * 1024), "32,0 GB");
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024 * 1024), "2,0 TB");
    }

    #[test]
    fn dmi() {
        assert_eq!(dmi_clean(Some("To Be Filled By O.E.M.".into())), None);
        assert_eq!(dmi_clean(Some("MS-7C95".into())), Some("MS-7C95".into()));
    }
}
