//! Mostra no terminal (em JSON) o que o app enxerga do hardware, sem precisar abrir a interface.
//! Uso: reset-probe [system|gpu|sensors|disks|usb|updates|drivers|apps|catalog|rgb [host:porta]]
//!       reset-probe rgb-set RRGGBB [segundos]   acende todos os LEDs nessa cor e depois devolve o efeito anterior

use reset_core::{apps, devices, disks, gpu, lighting::openrgb, packages, sensors::SensorMonitor, system};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let what = args.first().map(String::as_str).unwrap_or("all");
    let out = match what {
        "system" => serde_json::to_value(system::collect()),
        "gpu" => serde_json::to_value(gpu::list()),
        "sensors" => {
            let mut m = SensorMonitor::new();
            m.poll();
            std::thread::sleep(Duration::from_millis(500)); // carga da CPU precisa de duas leituras
            serde_json::to_value(m.poll())
        }
        "disks" => serde_json::to_value(disks::list()),
        "usb" => serde_json::to_value(devices::usb()),
        "updates" => match packages::check_updates() {
            Ok(u) => serde_json::to_value(u),
            Err(e) => Ok(serde_json::json!({ "erro": e })),
        },
        "drivers" => serde_json::to_value(packages::driver_overview()),
        // ícones só com --icons (são data URLs enormes no terminal)
        "apps" if args.get(1).is_some_and(|a| a == "--icons") => serde_json::to_value(apps::installed()),
        "apps" => serde_json::to_value(apps::installed().into_iter().map(|mut a| { a.icon = if a.icon.is_empty() { "-".into() } else { "ok".into() }; a }).collect::<Vec<_>>()),
        "catalog" => serde_json::to_value(apps::catalog()),
        "rgb" => {
            let target = args.get(1).map(String::as_str).unwrap_or("127.0.0.1");
            let (host, port) = target.split_once(':').map_or((target, openrgb::DEFAULT_PORT), |(h, p)| (h, p.parse().unwrap_or(openrgb::DEFAULT_PORT)));
            match openrgb::OpenRgb::connect(host, port) {
                Ok((_, devs)) => serde_json::to_value(devs.iter().map(|d| serde_json::json!({"id": d.id, "name": d.name, "kind": d.kind, "detail": d.detail, "zones": d.zones})).collect::<Vec<_>>()),
                Err(e) => Ok(serde_json::json!({ "erro": e })),
            }
        }
        "rgb-set" => {
            let hex = args.get(1).map(String::as_str).unwrap_or("00ff00");
            let color = [0, 2, 4].map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0));
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
            match openrgb::OpenRgb::connect("127.0.0.1", openrgb::DEFAULT_PORT) {
                Ok((mut c, devs)) => {
                    let mut log = Vec::new();
                    for d in &devs {
                        let r = c.acquire(&d.id).and_then(|_| c.send(&d.id, &vec![color; d.led_x.len()]));
                        log.push(serde_json::json!({ "dispositivo": d.name, "leds": d.led_x.len(), "resultado": r.err().unwrap_or_else(|| "ok".into()) }));
                    }
                    std::thread::sleep(Duration::from_secs(secs));
                    for d in &devs {
                        c.release(&d.id);
                    }
                    Ok(serde_json::json!({ "cor": color, "segundos": secs, "enviado": log }))
                }
                Err(e) => Ok(serde_json::json!({ "erro": e })),
            }
        }
        _ => Ok(serde_json::json!({
            "system": system::collect(),
            "gpu": gpu::list(),
            "disks": disks::list(),
            "usb": devices::usb(),
        })),
    };
    println!("{}", serde_json::to_string_pretty(&out.expect("json")).unwrap());
}

