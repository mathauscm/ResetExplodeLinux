//! Núcleo do RESET EXPLODE para Linux: tudo que lê ou mexe no hardware, sem nada de interface.
//! A interface (Tauri) só chama estas funções; o binário `reset-probe` usa as mesmas para testar no terminal.

pub mod apps;
pub mod devices;
pub mod disks;
pub mod gpu;
pub mod lighting;
pub mod packages;
pub mod sensors;
pub mod system;
pub mod util;
