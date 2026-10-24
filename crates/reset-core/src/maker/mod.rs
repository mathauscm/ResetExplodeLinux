//! RESET MAKER para Linux: portas seriais (Arduino, ESP32, conversores USB-serial) e os drivers delas.
//!
//! No Windows o trabalho era caçar drivers e números COM presos. No Linux os drivers de CH340, CP210x, FTDI,
//! PL2303 e CDC-ACM já vêm no kernel e o nome da porta nasce e morre com a conexão; os problemas reais são
//! outros: permissão (grupo `uucp`/`dialout`), o brltty capturando conversores CH340 e o ModemManager
//! "testando" placas como se fossem modem. É isso que estes módulos verificam.
//!
//! - `ports`: portas existentes, quem usa cada uma e a permissão do usuário
//! - `serial`: monitor serial (termios via libc)
//! - `drivers`: módulos do kernel por chip, brltty, regras udev e Arduino IDE

pub mod drivers;
pub mod ports;
pub mod serial;

use serde::Serialize;

/// Família de chip USB-serial (ou placa com USB nativo) e o módulo do kernel que cuida dela.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Chip {
    /// Identificador curto, usado pela interface: "ch340", "cp210x"...
    pub id: &'static str,
    pub name: &'static str,
    /// Módulo do kernel (com `_`, como aparece em /sys/module).
    pub module: &'static str,
    pub vid: u16,
    /// Produtos reconhecidos; vazio = qualquer produto deste fabricante.
    pub pids: &'static [u16],
}

/// Ordem importa: o primeiro que bate vence (CH9102 da WCH usa CDC-ACM, não o ch341).
pub const CHIPS: &[Chip] = &[
    Chip { id: "ch9102", name: "WCH CH9102/CH343", module: "cdc_acm", vid: 0x1a86, pids: &[0x55d2, 0x55d3, 0x55d4, 0x55d5] },
    Chip { id: "ch340", name: "WCH CH340/CH341", module: "ch341", vid: 0x1a86, pids: &[0x7523, 0x7522, 0x5523, 0x5512] },
    Chip { id: "cp210x", name: "Silicon Labs CP210x", module: "cp210x", vid: 0x10c4, pids: &[0xea60, 0xea61, 0xea63, 0xea70, 0xea71, 0xea80] },
    Chip { id: "ftdi", name: "FTDI", module: "ftdi_sio", vid: 0x0403, pids: &[] },
    Chip { id: "pl2303", name: "Prolific PL2303", module: "pl2303", vid: 0x067b, pids: &[] },
    Chip { id: "arduino", name: "Arduino (USB nativo)", module: "cdc_acm", vid: 0x2341, pids: &[] },
    Chip { id: "arduino", name: "Arduino (USB nativo)", module: "cdc_acm", vid: 0x2a03, pids: &[] },
    Chip { id: "espressif", name: "Espressif (USB nativo)", module: "cdc_acm", vid: 0x303a, pids: &[] },
    Chip { id: "rp2040", name: "Raspberry Pi RP2040/RP2350", module: "cdc_acm", vid: 0x2e8a, pids: &[] },
    Chip { id: "stm32", name: "STM32 (USB CDC)", module: "cdc_acm", vid: 0x0483, pids: &[0x5740] },
    Chip { id: "adafruit", name: "Adafruit (USB nativo)", module: "cdc_acm", vid: 0x239a, pids: &[] },
    Chip { id: "sparkfun", name: "SparkFun (USB nativo)", module: "cdc_acm", vid: 0x1b4f, pids: &[] },
];

pub fn chip_for(vid: u16, pid: u16) -> Option<&'static Chip> {
    CHIPS.iter().find(|c| c.vid == vid && (c.pids.is_empty() || c.pids.contains(&pid)))
}

/// Placa provável pelo VID:PID. Conversores (CH340, CP2102...) aparecem em muitas placas: o texto lista as comuns.
pub fn board_guess(vid: u16, pid: u16) -> Option<&'static str> {
    Some(match (vid, pid) {
        (0x2341 | 0x2a03, 0x0043 | 0x0001 | 0x0243) => "Arduino Uno",
        (0x2341 | 0x2a03, 0x0042 | 0x0010) => "Arduino Mega 2560",
        (0x2341 | 0x2a03, 0x8036 | 0x0036) => "Arduino Leonardo",
        (0x2341, 0x8037 | 0x0037) => "Arduino Micro",
        (0x2341, 0x0058) => "Arduino Nano Every",
        (0x2341, 0x0069) => "Arduino Uno R4 Minima",
        (0x2341, 0x1002) => "Arduino Uno R4 WiFi",
        (0x2341, 0x0070) => "Arduino Nano ESP32",
        (0x2341, 0x003d | 0x003e) => "Arduino Due",
        (0x2341, 0x805a) => "Arduino Nano 33 BLE",
        (0x2341, 0x8057) => "Arduino Nano 33 IoT",
        (0x2341 | 0x2a03, _) => "Placa Arduino",
        (0x1a86, 0x7523 | 0x7522) => "Arduino Nano/Uno compatível, NodeMCU v3, Wemos D1 mini ou ESP32 genérico",
        (0x1a86, 0x5523) => "Conversor CH341 (gravador/adaptador)",
        (0x1a86, 0x55d4 | 0x55d3) => "ESP32 DevKit (versões mais novas)",
        (0x10c4, 0xea60) => "ESP32 DevKit, NodeMCU v1.0 (ESP8266) ou conversor CP2102",
        (0x0403, 0x6001) => "Conversor FTDI (Pro Mini, Nano antigo, cabo FTDI)",
        (0x0403, 0x6015) => "Conversor FTDI FT231X",
        (0x0403, 0x6010 | 0x6011 | 0x6014) => "Conversor/depurador FTDI multiporta",
        (0x067b, _) => "Cabo ou conversor USB-serial (comum em cabos RS-232)",
        (0x303a, 0x1001) => "ESP32-S3/C3/C6 (USB-Serial/JTAG nativo)",
        (0x303a, 0x0002) => "ESP32-S2 (USB nativo)",
        (0x303a, _) => "Placa Espressif com USB nativo",
        (0x2e8a, 0x0003 | 0x000f) => "Raspberry Pi Pico em modo BOOTSEL",
        (0x2e8a, _) => "Raspberry Pi Pico",
        (0x0483, 0x5740) => "STM32 (Blue Pill/Black Pill) com USB CDC",
        (0x239a, _) => "Placa Adafruit",
        (0x1b4f, _) => "Placa SparkFun",
        _ => return None,
    })
}

/// Pico/RP2040 em BOOTSEL: aparece como pendrive (RPI-RP2), não como porta serial. Não é defeito.
pub fn is_bootsel(vid: u16, pid: u16) -> bool {
    vid == 0x2e8a && matches!(pid, 0x0003 | 0x000f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips() {
        assert_eq!(chip_for(0x1a86, 0x7523).unwrap().module, "ch341");
        assert_eq!(chip_for(0x1a86, 0x55d4).unwrap().id, "ch9102");
        assert_eq!(chip_for(0x10c4, 0xea60).unwrap().module, "cp210x");
        assert_eq!(chip_for(0x0403, 0x6015).unwrap().module, "ftdi_sio");
        assert_eq!(chip_for(0x303a, 0x1001).unwrap().module, "cdc_acm");
        assert!(chip_for(0x046d, 0xc52b).is_none());
        // CP210x de outro produto da Silicon Labs (não serial) não conta
        assert!(chip_for(0x10c4, 0x8105).is_none());
        assert_eq!(board_guess(0x2341, 0x0043), Some("Arduino Uno"));
        assert_eq!(board_guess(0x2341, 0x9999), Some("Placa Arduino"));
        assert!(is_bootsel(0x2e8a, 0x0003));
    }
}
