//! Janela do RESET EXPLODE para Linux. Toda a lógica fica no `reset-core`; aqui só ficam os comandos
//! que a interface (app/ui) chama com `invoke`.

mod areas;
mod jobs;

use reset_core::lighting::{self, ColorOrder, LightEnv, LightSettings, LightStatus, Lighting};
use reset_core::sensors::{SensorMonitor, Snapshot};
use reset_core::packages::{self, PkgAction};
use reset_core::{apps, devices, disks, gpu, system};
use std::sync::Mutex;
use tauri::{AppHandle, State};

struct Sensors(Mutex<SensorMonitor>);

#[tauri::command]
fn system_info() -> system::SystemInfo {
    system::collect()
}

#[tauri::command]
fn gpu_list() -> Vec<gpu::GpuInfo> {
    gpu::list()
}

#[tauri::command]
fn sensors_poll(s: State<Sensors>) -> Snapshot {
    s.0.lock().unwrap().poll()
}

#[tauri::command]
fn sensors_reset(s: State<Sensors>) {
    s.0.lock().unwrap().reset_extremes();
}

#[tauri::command]
fn disks_list() -> Vec<disks::Disk> {
    disks::list()
}

#[tauri::command]
fn usb_list() -> Vec<devices::UsbDevice> {
    devices::usb()
}

// Os comandos de iluminação conversam com o OpenRGB pela rede (até 2 s de espera): `async` os tira do thread da janela.

#[tauri::command]
async fn light_ensure(l: State<'_, Lighting>, host: String, port: u16) -> Result<LightStatus, ()> {
    Ok(l.ensure(&host, port))
}

#[tauri::command]
async fn light_resize(l: State<'_, Lighting>, device: String, zone: u32, size: u32) -> Result<LightStatus, ()> {
    Ok(l.resize_zone(&device, zone, size))
}

#[tauri::command]
async fn light_order(l: State<'_, Lighting>, device: String, zone: u32, order: ColorOrder) -> Result<LightStatus, ()> {
    Ok(l.set_order(&device, zone, order))
}

#[tauri::command]
async fn light_apply(l: State<'_, Lighting>, settings: LightSettings) -> Result<LightStatus, ()> {
    Ok(l.apply(settings))
}

#[tauri::command]
async fn light_start(l: State<'_, Lighting>) -> Result<LightStatus, ()> {
    Ok(l.start())
}

#[tauri::command]
async fn light_stop(l: State<'_, Lighting>) -> Result<LightStatus, ()> {
    Ok(l.stop())
}

#[tauri::command]
fn light_status(l: State<Lighting>) -> LightStatus {
    l.status()
}

#[tauri::command]
async fn light_identify(l: State<'_, Lighting>, device: String, zone: usize) -> Result<LightStatus, ()> {
    Ok(l.identify(&device, zone))
}

#[tauri::command]
fn light_env() -> LightEnv {
    lighting::environment()
}

#[tauri::command]
fn light_autostart() -> bool {
    lighting::autostart_enabled()
}

#[tauri::command]
fn light_set_autostart(on: bool) -> Result<bool, String> {
    lighting::set_autostart(on).map(|_| on)
}

// ---------------------------------------------------------------- drivers e apps

#[tauri::command]
async fn updates_check() -> Result<Vec<packages::Update>, String> {
    packages::check_updates()
}

#[tauri::command]
async fn drivers_overview() -> Result<packages::DriverOverview, ()> {
    Ok(packages::driver_overview())
}

#[tauri::command]
async fn apps_installed() -> Result<Vec<apps::App>, ()> {
    Ok(apps::installed())
}

#[tauri::command]
async fn apps_catalog() -> Result<Vec<apps::CatalogApp>, ()> {
    Ok(apps::catalog())
}

/// Roda o pacman com root (atualizar, instalar, remover). A saída vai para a interface pelos eventos de `jobs`.
#[tauri::command]
fn pkg_run(app: AppHandle, action: PkgAction) -> Result<(), String> {
    let args = packages::pacman_args(&action)?;
    if packages::pacman_locked() {
        return Err("outro programa está instalando ou atualizando pacotes agora. Espere ele terminar e tente de novo.".into());
    }
    jobs::spawn(&app, true, packages::PACMAN, &args)
}

/// Modo sem janela usado pelo login (`--apply-lights`): conecta, reaplica o efeito salvo e sai. Efeitos
/// animados (arco-íris, temperatura...) precisam continuar calculando, então nesse caso fica rodando.
fn apply_lights_headless() {
    let l = Lighting::new();
    let s = l.ensure("127.0.0.1", lighting::openrgb::DEFAULT_PORT);
    if !s.running {
        return;
    }
    if lighting::is_animated(s.settings.effect) {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    std::thread::sleep(std::time::Duration::from_secs(3)); // tempo de o motor enviar as cores
}

/// A janela assume os LEDs: encerra um `--apply-lights` que tenha ficado rodando um efeito animado,
/// senão os dois brigariam pelas cores.
fn stop_headless_instances() {
    let me = std::process::id();
    let Ok(dir) = std::fs::read_dir("/proc") else { return };
    for e in dir.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if pid == me {
            continue;
        }
        let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
        let args: Vec<&[u8]> = cmd.split(|&b| b == 0).collect();
        let ours = args.first().is_some_and(|a| a.ends_with(b"reset-explode"));
        if ours && args.iter().any(|a| *a == b"--apply-lights") {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }
}

fn main() {
    // modo sem janela da tela Overclock: rodado via pkexec, aplica os ajustes da GPU (NVML) e sai
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == reset_core::oc::CLI_FLAG) {
        std::process::exit(reset_core::oc::cli_main(&args[i + 1..]));
    }
    if std::env::args().any(|a| a == "--apply-lights") {
        return apply_lights_headless();
    }
    stop_headless_instances();
    let lights = Lighting::new();
    tauri::Builder::default()
        .manage(Sensors(Mutex::new(SensorMonitor::new())))
        .manage(lights)
        .manage(jobs::JobLock(Mutex::new(false)))
        .invoke_handler(tauri::generate_handler![
            system_info,
            gpu_list,
            sensors_poll,
            sensors_reset,
            disks_list,
            usb_list,
            light_ensure,
            light_resize,
            light_order,
            light_apply,
            light_start,
            light_stop,
            light_status,
            light_env,
            light_identify,
            light_autostart,
            light_set_autostart,
            updates_check,
            drivers_overview,
            apps_installed,
            apps_catalog,
            pkg_run,
            areas::area_call,
        ])
        // ao fechar, os LEDs ficam como estão: as cores escolhidas valem até o próximo desligamento
        // (e voltam no login, se a opção estiver ligada)
        .run(tauri::generate_context!())
        .expect("erro ao abrir o RESET EXPLODE");
}
