//! Atualizações e drivers pelo pacman (Arch/CachyOS). Ler não precisa de root: `checkupdates` sincroniza
//! uma cópia temporária do banco. Instalar/remover/atualizar roda com `pkexec`, que mostra a janela de senha
//! do sistema; os comandos são montados aqui, a partir de nomes de pacote validados, nunca vindos prontos da interface.

use crate::gpu;
use crate::util::read_trim;
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct Update {
    pub name: String,
    pub from: String,
    pub to: String,
    /// "Kernel", "Placa de vídeo (NVIDIA)", "Gráficos (Mesa e Vulkan)", "Firmware e microcódigo", "Outros"
    pub category: String,
    /// Pacotes com o mesmo grupo têm que ser atualizados juntos (kernel + módulo NVIDIA + driver NVIDIA,
    /// mesa + lib32-mesa...). A interface marca e desmarca o grupo inteiro.
    pub group: usize,
}

pub const CAT_KERNEL: &str = "Kernel";
pub const CAT_NVIDIA: &str = "Placa de vídeo (NVIDIA)";
pub const CAT_MESA: &str = "Gráficos (Mesa e Vulkan)";
pub const CAT_FIRMWARE: &str = "Firmware e microcódigo";
pub const CAT_OTHER: &str = "Outros pacotes do sistema";

/// Kernel: "linux-cachyos-lts-nvidia-open" -> Some("linux-cachyos-lts"). Firmware e headers da API não contam.
fn kernel_base(name: &str) -> Option<String> {
    if !name.starts_with("linux") || name.starts_with("linux-firmware") || name == "linux-api-headers" {
        return None;
    }
    let mut base = name.to_string();
    for suffix in ["-headers", "-nvidia-open", "-nvidia", "-zfs", "-docs"] {
        if let Some(b) = base.strip_suffix(suffix) {
            base = b.to_string();
        }
    }
    Some(base)
}

/// "linux-firmware-nvidia" é firmware, não driver: confere antes de olhar "nvidia" no nome.
fn is_firmware(name: &str) -> bool {
    name.starts_with("linux-firmware") || name.ends_with("-ucode") || name.ends_with("-firmware")
}

pub fn category(name: &str) -> &'static str {
    let n = name.strip_prefix("lib32-").unwrap_or(name);
    if is_firmware(n) {
        CAT_FIRMWARE
    } else if kernel_base(name).is_some() {
        CAT_KERNEL
    } else if n.contains("nvidia") {
        CAT_NVIDIA
    } else if n == "mesa" || n.starts_with("mesa-") || n.starts_with("vulkan-") || n == "libdrm" || n.starts_with("libva") {
        CAT_MESA
    } else {
        CAT_OTHER
    }
}

/// Chaves que amarram pacotes: quem compartilha uma chave vai junto.
fn link_keys(name: &str) -> Vec<String> {
    let mut keys = vec![format!("pkg:{}", name.strip_prefix("lib32-").unwrap_or(name))];
    if let Some(base) = kernel_base(name) {
        keys.push(format!("kernel:{base}"));
    }
    if name.contains("nvidia") && !is_firmware(name) {
        // o módulo do kernel e o driver em espaço de usuário têm que ser da mesma versão
        keys.push("nvidia".into());
    }
    if is_firmware(name) && name.starts_with("linux-firmware") {
        keys.push("firmware".into());
    }
    keys
}

/// Agrupa por chaves em comum (união de conjuntos simples; são poucas dezenas de pacotes).
fn assign_groups(names: &[String]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..names.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (i, n) in names.iter().enumerate() {
        for k in link_keys(n) {
            match owner.get(&k) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                }
                None => {
                    owner.insert(k, i);
                }
            }
        }
    }
    (0..names.len()).map(|i| find(&mut parent, i)).collect()
}

pub fn check_updates() -> Result<Vec<Update>, String> {
    let out = Command::new("checkupdates")
        .env("LC_ALL", "C")
        .output()
        .map_err(|_| "o comando checkupdates não foi encontrado (pacote pacman-contrib).".to_string())?;
    // código 2 = nenhuma atualização
    if out.status.code() == Some(2) {
        return Ok(Vec::new());
    }
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("não deu para buscar atualizações: {}", err.trim()));
    }
    Ok(parse_checkupdates(&String::from_utf8_lossy(&out.stdout)))
}

/// Linhas "nome versão-atual -> versão-nova".
fn parse_checkupdates(text: &str) -> Vec<Update> {
    let rows: Vec<(String, String, String)> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (name, from, arrow, to) = (it.next()?, it.next()?, it.next()?, it.next()?);
            (arrow == "->").then(|| (name.to_string(), from.to_string(), to.to_string()))
        })
        .collect();
    let names: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let groups = assign_groups(&names);
    rows.into_iter()
        .zip(groups)
        .map(|((name, from, to), group)| Update { category: category(&name).to_string(), name, from, to, group })
        .collect()
}

/// Resumo dos drivers em uso, para o topo da tela.
#[derive(Debug, Clone, Serialize)]
pub struct DriverOverview {
    pub gpu: Option<String>,
    pub nvidia_driver: Option<String>,
    /// "aberto" (nvidia-open) ou "proprietário"
    pub nvidia_module: Option<String>,
    pub kernel_running: String,
    /// Kernels instalados (nome, versão).
    pub kernels: Vec<(String, String)>,
    /// Há um kernel mais novo instalado do que o que está rodando: falta reiniciar.
    pub reboot_needed: bool,
    pub mesa: Option<String>,
    pub firmware: Option<String>,
    pub microcode: Option<String>,
    /// Pacotes de driver instalados (nome, versão), para a lista "o que tenho".
    pub packages: Vec<(String, String, String)>,
}

/// `pacman -Q`: (nome, versão) de tudo que está instalado.
pub fn installed_versions() -> HashMap<String, String> {
    let out = Command::new("pacman").arg("-Q").env("LC_ALL", "C").output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split_once(' ').map(|(n, v)| (n.to_string(), v.to_string())))
        .collect()
}

pub fn driver_overview() -> DriverOverview {
    let q = installed_versions();
    let kernel_running = read_trim("/proc/sys/kernel/osrelease").unwrap_or_default();
    let mut kernels: Vec<(String, String)> = q
        .iter()
        .filter(|(n, _)| kernel_base(n).as_deref() == Some(n.as_str()))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    kernels.sort();
    // "7.3.0-4-cachyos" rodando x pacote "linux-cachyos 7.3.0-4": compara o começo da versão
    let reboot_needed = !kernels.is_empty() && !kernels.iter().any(|(_, v)| kernel_running.starts_with(v.as_str()));
    let nvidia_module = if read_trim("/sys/module/nvidia/version").is_none() {
        None
    } else if q.keys().any(|n| n.contains("nvidia-open")) {
        Some("aberto (nvidia-open)".into())
    } else {
        Some("proprietário".into())
    };
    let mut packages: Vec<(String, String, String)> = q
        .iter()
        .filter(|(n, _)| category(n) != CAT_OTHER)
        .map(|(n, v)| (category(n).to_string(), n.clone(), v.clone()))
        .collect();
    packages.sort();
    DriverOverview {
        gpu: gpu::names().first().map(|(_, n)| n.clone()),
        nvidia_driver: gpu::list().first().and_then(|g| g.driver.clone()),
        nvidia_module,
        kernel_running,
        kernels,
        reboot_needed,
        mesa: q.get("mesa").cloned(),
        firmware: q.get("linux-firmware").cloned(),
        microcode: q.get("amd-ucode").or_else(|| q.get("intel-ucode")).cloned(),
        packages,
    }
}

/// Nome de pacote seguro para ir na linha de comando (o padrão de nomes do Arch).
pub fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
}

/// O que um trabalho com root vai fazer. Cada variante vira um comando fixo do pacman.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PkgAction {
    /// Atualiza o sistema, deixando de fora os pacotes em `ignore`.
    Upgrade { ignore: Vec<String> },
    /// Instala (com o sistema atualizado junto: o Arch não suporta instalar com o banco velho).
    Install { packages: Vec<String> },
    /// Remove com as dependências que só ele usava.
    Remove { packages: Vec<String> },
}

pub const PACMAN: &str = "/usr/bin/pacman";

/// Argumentos do pacman para a ação (o app chama `pkexec /usr/bin/pacman <args>`).
pub fn pacman_args(action: &PkgAction) -> Result<Vec<String>, String> {
    let check = |list: &[String]| match list.iter().find(|p| !valid_package(p)) {
        Some(bad) => Err(format!("nome de pacote inválido: {bad}")),
        None => Ok(()),
    };
    let base = ["--noconfirm", "--noprogressbar", "--color", "never"].map(String::from);
    let mut args: Vec<String>;
    match action {
        PkgAction::Upgrade { ignore } => {
            check(ignore)?;
            args = vec!["-Syu".into()];
            args.extend(base);
            if !ignore.is_empty() {
                args.push("--ignore".into());
                args.push(ignore.join(","));
            }
        }
        PkgAction::Install { packages } => {
            check(packages)?;
            if packages.is_empty() {
                return Err("nenhum pacote escolhido".into());
            }
            args = vec!["-Syu".into(), "--needed".into()];
            args.extend(base);
            args.extend(packages.iter().cloned());
        }
        PkgAction::Remove { packages } => {
            check(packages)?;
            if packages.is_empty() {
                return Err("nenhum pacote escolhido".into());
            }
            args = vec!["-Rns".into()];
            args.extend(base);
            args.extend(packages.iter().cloned());
        }
    }
    Ok(args)
}

/// Outro programa (pamac, terminal com pacman...) está mexendo nos pacotes agora.
pub fn pacman_locked() -> bool {
    std::path::Path::new("/var/lib/pacman/db.lck").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorias() {
        assert_eq!(category("linux-cachyos"), CAT_KERNEL);
        assert_eq!(category("linux-cachyos-nvidia-open"), CAT_KERNEL);
        assert_eq!(category("lib32-nvidia-utils"), CAT_NVIDIA);
        assert_eq!(category("lib32-mesa"), CAT_MESA);
        assert_eq!(category("linux-firmware-nvidia"), CAT_FIRMWARE);
        assert_eq!(category("amd-ucode"), CAT_FIRMWARE);
        assert_eq!(category("linux-api-headers"), CAT_OTHER);
        assert_eq!(category("pkgfile"), CAT_OTHER);
    }

    #[test]
    fn grupos_que_andam_juntos() {
        let u = parse_checkupdates(
            "linux-cachyos 7.3.0-4 -> 7.3.1-1\nlinux-cachyos-headers 7.3.0-4 -> 7.3.1-1\nlinux-cachyos-nvidia-open 7.3.0-4 -> 7.3.1-1\n\
             nvidia-utils 615.78.08-2 -> 615.80-1\nlib32-nvidia-utils 615.78.08-1 -> 615.80-1\nmesa 26.2.5-1 -> 26.2.6-1\n\
             lib32-mesa 26.2.5-1 -> 26.2.6-1\npkgfile 26-1.1 -> 27-1.1\nlinux-cachyos-lts 6.18.55-2 -> 6.18.56-1\n",
        );
        let g = |n: &str| u.iter().find(|x| x.name == n).unwrap().group;
        // kernel, headers, módulo NVIDIA e driver NVIDIA: tudo junto
        assert_eq!(g("linux-cachyos"), g("linux-cachyos-headers"));
        assert_eq!(g("linux-cachyos"), g("nvidia-utils"));
        assert_eq!(g("nvidia-utils"), g("lib32-nvidia-utils"));
        assert_eq!(g("mesa"), g("lib32-mesa"));
        assert_ne!(g("mesa"), g("pkgfile"));
        assert_ne!(g("linux-cachyos"), g("mesa"));
        // o LTS sem módulo NVIDIA na lista anda sozinho
        assert_ne!(g("linux-cachyos-lts"), g("linux-cachyos"));
    }

    #[test]
    fn comandos() {
        let a = pacman_args(&PkgAction::Upgrade { ignore: vec!["mesa".into(), "lib32-mesa".into()] }).unwrap();
        assert_eq!(a.first().unwrap(), "-Syu");
        assert_eq!(a.last().unwrap(), "mesa,lib32-mesa");
        assert!(pacman_args(&PkgAction::Remove { packages: vec!["--root=/".into()] }).is_err());
        assert!(pacman_args(&PkgAction::Install { packages: vec!["vlc; rm -rf /".into()] }).is_err());
        assert!(valid_package("gtk3"));
        assert!(valid_package("lib32-nvidia-utils"));
        assert!(valid_package("libc++"));
    }
}
