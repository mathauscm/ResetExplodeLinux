//! Especificações de referência que o hardware não reporta (como o banco de dados do GPU-Z). Portado do GpuSpecDb.cs.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct GpuSpec {
    pub chip: &'static str,
    pub arch: &'static str,
    pub process: &'static str,
    pub die_mm2: u32,
    pub transistors_bi: f64,
    pub shaders: u32,
    pub rops: u32,
    pub tmus: u32,
    pub mem_type: &'static str,
    pub base_mhz: u32,
    pub boost_mhz: u32,
    pub tdp_w: u32,
    pub release: &'static str,
    /// Mesma ID PCI com variantes: o trecho do nome que desempata.
    #[serde(skip)]
    pub name_match: Option<&'static str>,
}

#[allow(clippy::too_many_arguments)]
const fn s(
    chip: &'static str, arch: &'static str, process: &'static str, die_mm2: u32, transistors_bi: f64, shaders: u32, rops: u32,
    tmus: u32, mem_type: &'static str, base_mhz: u32, boost_mhz: u32, tdp_w: u32, release: &'static str,
) -> GpuSpec {
    GpuSpec { chip, arch, process, die_mm2, transistors_bi, shaders, rops, tmus, mem_type, base_mhz, boost_mhz, tdp_w, release, name_match: None }
}

const fn m(spec: GpuSpec, name_match: &'static str) -> GpuSpec {
    GpuSpec { name_match: Some(name_match), ..spec }
}

const N4: &str = "TSMC 4N (5 nm)";
const S8: &str = "Samsung 8 nm";
const N5: &str = "TSMC 5 nm + 6 nm";

/// Chave: "VVVV:DDDD" (vendor:device PCI).
const DB: &[(&str, &[GpuSpec])] = &[
    // ---- NVIDIA Blackwell (RTX 50) ----
    ("10DE:2B85", &[s("GB202", "Blackwell", N4, 750, 92.2, 21760, 176, 680, "GDDR7", 2017, 2407, 575, "30/01/2025")]),
    ("10DE:2C02", &[s("GB203", "Blackwell", N4, 378, 45.6, 10752, 112, 336, "GDDR7", 2295, 2617, 360, "30/01/2025")]),
    ("10DE:2C05", &[s("GB203", "Blackwell", N4, 378, 45.6, 8960, 96, 280, "GDDR7", 2295, 2452, 300, "20/02/2025")]),
    ("10DE:2F04", &[s("GB205", "Blackwell", N4, 263, 31.1, 6144, 80, 192, "GDDR7", 2325, 2512, 250, "05/03/2025")]),
    ("10DE:2D04", &[s("GB206", "Blackwell", N4, 181, 21.9, 4608, 48, 144, "GDDR7", 2407, 2572, 180, "16/04/2025")]),
    ("10DE:2D05", &[s("GB206", "Blackwell", N4, 181, 21.9, 3840, 48, 120, "GDDR7", 2280, 2497, 145, "19/05/2025")]),
    // ---- NVIDIA Ada Lovelace (RTX 40) ----
    ("10DE:2684", &[s("AD102", "Ada Lovelace", N4, 609, 76.3, 16384, 176, 512, "GDDR6X", 2235, 2520, 450, "12/10/2022")]),
    ("10DE:2702", &[s("AD103", "Ada Lovelace", N4, 379, 45.9, 10240, 112, 320, "GDDR6X", 2295, 2550, 320, "31/01/2024")]),
    ("10DE:2704", &[s("AD103", "Ada Lovelace", N4, 379, 45.9, 9728, 112, 304, "GDDR6X", 2205, 2505, 320, "16/11/2022")]),
    ("10DE:2705", &[s("AD103", "Ada Lovelace", N4, 379, 45.9, 8448, 96, 264, "GDDR6X", 2340, 2610, 285, "24/01/2024")]),
    ("10DE:2782", &[s("AD104", "Ada Lovelace", N4, 295, 35.8, 7680, 80, 240, "GDDR6X", 2310, 2610, 285, "05/01/2023")]),
    ("10DE:2783", &[s("AD104", "Ada Lovelace", N4, 295, 35.8, 7168, 80, 224, "GDDR6X", 1980, 2475, 220, "17/01/2024")]),
    ("10DE:2786", &[s("AD104", "Ada Lovelace", N4, 295, 35.8, 5888, 64, 184, "GDDR6X", 1920, 2475, 200, "13/04/2023")]),
    ("10DE:2803", &[s("AD106", "Ada Lovelace", N4, 190, 22.9, 4352, 48, 136, "GDDR6", 2310, 2535, 160, "24/05/2023")]),
    ("10DE:2805", &[s("AD106", "Ada Lovelace", N4, 190, 22.9, 4352, 48, 136, "GDDR6", 2310, 2535, 165, "18/07/2023")]),
    ("10DE:2882", &[s("AD107", "Ada Lovelace", N4, 159, 18.9, 3072, 48, 96, "GDDR6", 1830, 2460, 115, "29/06/2023")]),
    // ---- NVIDIA Ampere (RTX 30) ----
    ("10DE:2204", &[s("GA102", "Ampere", S8, 628, 28.3, 10496, 112, 328, "GDDR6X", 1395, 1695, 350, "24/09/2020")]),
    ("10DE:2206", &[s("GA102", "Ampere", S8, 628, 28.3, 8704, 96, 272, "GDDR6X", 1440, 1710, 320, "17/09/2020")]),
    ("10DE:2484", &[s("GA104", "Ampere", S8, 392, 17.4, 5888, 96, 184, "GDDR6", 1500, 1725, 220, "29/10/2020")]),
    ("10DE:2486", &[s("GA104", "Ampere", S8, 392, 17.4, 4864, 80, 152, "GDDR6", 1410, 1665, 200, "02/12/2020")]),
    ("10DE:2503", &[s("GA106", "Ampere", S8, 276, 12.0, 3584, 48, 112, "GDDR6", 1320, 1777, 170, "25/02/2021")]),
    ("10DE:2504", &[s("GA106", "Ampere", S8, 276, 12.0, 3584, 48, 112, "GDDR6", 1320, 1777, 170, "25/02/2021")]),
    // ---- AMD RDNA 4 / 3 / 2 ----
    ("1002:7550", &[
        m(s("Navi 48", "RDNA 4", "TSMC N4P (4 nm)", 357, 53.9, 4096, 128, 256, "GDDR6", 2400, 2970, 304, "06/03/2025"), "9070 XT"),
        m(s("Navi 48", "RDNA 4", "TSMC N4P (4 nm)", 357, 53.9, 3584, 128, 224, "GDDR6", 2070, 2520, 220, "06/03/2025"), "9070"),
    ]),
    ("1002:744C", &[
        m(s("Navi 31", "RDNA 3", N5, 529, 57.7, 6144, 192, 384, "GDDR6", 1855, 2499, 355, "13/12/2022"), "XTX"),
        m(s("Navi 31", "RDNA 3", N5, 529, 57.7, 5376, 192, 336, "GDDR6", 1500, 2394, 315, "13/12/2022"), "7900 XT"),
    ]),
    ("1002:747E", &[
        m(s("Navi 32", "RDNA 3", N5, 346, 28.1, 3840, 96, 240, "GDDR6", 1295, 2430, 263, "06/09/2023"), "7800"),
        m(s("Navi 32", "RDNA 3", N5, 346, 28.1, 3456, 96, 216, "GDDR6", 1435, 2544, 245, "06/09/2023"), "7700"),
    ]),
    ("1002:7480", &[s("Navi 33", "RDNA 3", "TSMC 6 nm", 204, 13.3, 2048, 64, 128, "GDDR6", 1720, 2655, 165, "25/05/2023")]),
    ("1002:164E", &[s("Raphael (iGPU)", "RDNA 2", "TSMC 6 nm", 122, 3.4, 128, 4, 8, "Memória do sistema (DDR5)", 400, 2200, 0, "27/09/2022")]),
];

/// Recursos por arquitetura, para a linha "Tecnologias".
pub fn arch_features(arch: &str) -> Option<&'static str> {
    Some(match arch {
        "Blackwell" => "DLSS 4 (Multi Frame Generation) · Ray Tracing 4ª ger. · Tensor 5ª ger. · Reflex 2 · NVENC 9ª ger.",
        "Ada Lovelace" => "DLSS 3 (Frame Generation) · Ray Tracing 3ª ger. · Tensor 4ª ger. · Reflex · NVENC AV1",
        "Ampere" => "DLSS 2 · Ray Tracing 2ª ger. · Tensor 3ª ger. · Reflex · Resizable BAR",
        "RDNA 4" => "FSR 4 · Ray Accelerators 3ª ger. · AI Accelerators · Anti-Lag 2 · AV1",
        "RDNA 3" => "FSR 3 · Ray Accelerators 2ª ger. · AI Accelerators · AV1",
        "RDNA 2" => "FSR · Ray Accelerators 1ª ger.",
        _ => return None,
    })
}

/// `pci_id` no formato "VVVV:DDDD" (o mesmo de `gpu::GpuInfo::pci_device_id`).
pub fn find(pci_id: &str, name: &str) -> Option<GpuSpec> {
    let (_, specs) = DB.iter().find(|(k, _)| k.eq_ignore_ascii_case(pci_id))?;
    let low = name.to_lowercase();
    specs.iter().find(|s| s.name_match.is_some_and(|m| low.contains(&m.to_lowercase()))).or(specs.first()).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtx_4060() {
        let s = find("10de:2882", "NVIDIA GeForce RTX 4060").unwrap();
        assert_eq!((s.chip, s.shaders, s.tdp_w), ("AD107", 3072, 115));
        assert!(arch_features(s.arch).unwrap().contains("Frame Generation"));
    }

    #[test]
    fn variants_by_name() {
        assert_eq!(find("1002:744C", "AMD Radeon RX 7900 XTX").unwrap().shaders, 6144);
        assert_eq!(find("1002:744C", "AMD Radeon RX 7900 XT").unwrap().shaders, 5376);
        assert_eq!(find("1002:7550", "AMD Radeon RX 9070").unwrap().shaders, 3584);
        assert!(find("10DE:0000", "?").is_none());
    }
}
