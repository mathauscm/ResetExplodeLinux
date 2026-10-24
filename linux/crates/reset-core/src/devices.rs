//! Dispositivos USB conectados e quais deles têm LEDs que o app sabe controlar.
//! Serve de "diagnóstico" da tela de iluminação: mostra o que existe no PC antes mesmo de ligar o OpenRGB.

use crate::util::{read_hex, read_trim};
use serde::Serialize;
use std::fs;

#[derive(Debug, Clone, Serialize)]
pub struct UsbDevice {
    pub vid: String,
    pub pid: String,
    pub vendor: String,
    pub product: String,
    pub rgb: Option<RgbHint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RgbHint {
    /// "Placa-mãe MSI (Mystic Light)", "Razer"...
    pub kind: String,
    /// Como o app controla: "OpenRGB", "OpenRGB ou OpenRazer"...
    pub via: String,
}

/// Fabricantes com LEDs controláveis pelo OpenRGB via USB. O OpenRGB confirma o modelo exato ao conectar.
const RGB_VENDORS: &[(u32, &str, &str)] = &[
    (0x1462, "MSI", "OpenRGB"),
    (0x1532, "Razer", "OpenRGB ou OpenRazer"),
    (0x1b1c, "Corsair", "OpenRGB"),
    (0x046d, "Logitech G", "OpenRGB"),
    (0x0b05, "ASUS Aura", "OpenRGB"),
    (0x048d, "Gigabyte RGB Fusion", "OpenRGB"),
    (0x1044, "Gigabyte", "OpenRGB"),
    (0x2516, "Cooler Master", "OpenRGB"),
    (0x1e71, "NZXT", "OpenRGB"),
    (0x0951, "HyperX", "OpenRGB"),
    (0x03f0, "HyperX (HP)", "OpenRGB"),
    (0x1038, "SteelSeries", "OpenRGB"),
    (0x3434, "Keychron", "OpenRGB"),
    (0x320f, "Glorious / Evision", "OpenRGB"),
    (0x258a, "Redragon / genérico", "OpenRGB (alguns modelos)"),
    (0x1a2c, "Teclado genérico RGB", "OpenRGB (alguns modelos)"),
];

pub fn usb() -> Vec<UsbDevice> {
    let Ok(dir) = fs::read_dir("/sys/bus/usb/devices") else { return Vec::new() };
    let mut out: Vec<UsbDevice> = dir
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let vid = read_hex(p.join("idVendor"))?;
            let pid = read_hex(p.join("idProduct"))?;
            if vid == 0x1d6b {
                return None; // root hubs do próprio Linux
            }
            let (mut vendor, mut product) = (read_trim(p.join("manufacturer")), read_trim(p.join("product")));
            if vendor.is_none() || product.is_none() {
                let ids = usb_ids_lookup(vid, pid);
                vendor = vendor.or(ids.0);
                product = product.or(ids.1);
            }
            let vendor = vendor.unwrap_or_else(|| format!("{vid:04x}"));
            let product = product.unwrap_or_else(|| format!("{pid:04x}"));
            let rgb = rgb_hint(vid, &product);
            Some(UsbDevice { vid: format!("{vid:04x}"), pid: format!("{pid:04x}"), vendor, product, rgb })
        })
        .collect();
    out.sort_by(|a, b| b.rgb.is_some().cmp(&a.rgb.is_some()).then(a.vendor.cmp(&b.vendor)));
    out.dedup_by(|a, b| a.vid == b.vid && a.pid == b.pid);
    out
}

fn rgb_hint(vid: u32, product: &str) -> Option<RgbHint> {
    let (_, brand, via) = RGB_VENDORS.iter().find(|(v, _, _)| *v == vid)?;
    let p = product.to_lowercase();
    // MSI também faz Wi-Fi/Bluetooth e hubs: só o controlador Mystic Light interessa
    if vid == 0x1462 && !p.contains("mystic") {
        return None;
    }
    // Logitech: receptores e mouses sem LED (MX, Unifying) não contam
    if vid == 0x046d && (p.contains("unifying") || (p.contains("receiver") && !p.contains("lightspeed"))) {
        return None;
    }
    let kind = if vid == 0x1462 { "Placa-mãe MSI (Mystic Light)".to_string() } else { format!("{brand} · {product}") };
    Some(RgbHint { kind, via: via.to_string() })
}

/// Nomes do banco /usr/share/hwdata/usb.ids, para aparelhos que não informam fabricante/produto.
fn usb_ids_lookup(vid: u32, pid: u32) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string("/usr/share/hwdata/usb.ids") else { return (None, None) };
    let (vk, pk) = (format!("{vid:04x}  "), format!("\t{pid:04x}  "));
    let mut vendor = None;
    for l in text.lines() {
        if vendor.is_none() {
            if let Some(n) = l.strip_prefix(&vk) {
                vendor = Some(n.trim().to_string());
            }
        } else if let Some(n) = l.strip_prefix(&pk) {
            return (vendor, Some(n.trim().to_string()));
        } else if !l.starts_with('\t') && !l.starts_with('#') && !l.is_empty() {
            break; // acabou a seção do fabricante
        }
    }
    (vendor, None)
}
