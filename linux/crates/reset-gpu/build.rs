//! Compila os shaders GLSL de `shaders/` para SPIR-V com o glslc (pacote `shaderc`), na hora do build.
//! O binário embute o SPIR-V: em tempo de execução não precisa de compilador nenhum.
//! Outro glslc pode ser indicado pela variável GLSLC.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// (arquivo-fonte, saída .spv, defines). Um mesmo arquivo vira vários shaders, um por define.
const SHADERS: &[(&str, &str, &[&str])] = &[
    ("scene.vert", "scene_terrain.vert", &["TERRAIN"]),
    ("scene.vert", "scene_tree.vert", &["TREE"]),
    ("scene.vert", "scene_water.vert", &["WATER"]),
    ("scene.vert", "scene_shadow_terrain.vert", &["SHADOW_TERRAIN"]),
    ("scene.vert", "scene_shadow_tree.vert", &["SHADOW_TREE"]),
    ("scene.frag", "scene_terrain.frag", &["TERRAIN"]),
    ("scene.frag", "scene_tree.frag", &["TREE"]),
    ("scene.frag", "scene_water.frag", &["WATER"]),
    ("scene.frag", "scene_sky.frag", &["SKY"]),
    ("scene.frag", "scene_post.frag", &["POST"]),
    ("fullscreen.vert", "fullscreen.vert", &[]),
    ("rt.comp", "rt.comp", &[]),
    ("burn.vert", "burn.vert", &[]),
    ("burn.frag", "burn.frag", &[]),
];

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let glslc = env::var("GLSLC").unwrap_or_else(|_| "glslc".into());
    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-env-changed=GLSLC");

    for (src, name, defines) in SHADERS {
        let mut cmd = Command::new(&glslc);
        cmd.arg("--target-env=vulkan1.3").arg("-O").arg("-I").arg(&dir);
        for d in *defines {
            cmd.arg(format!("-D{d}"));
        }
        cmd.arg(dir.join(src)).arg("-o").arg(out.join(format!("{name}.spv")));
        let res = cmd.output().unwrap_or_else(|e| {
            panic!("não consegui rodar o glslc ({e}). Instale o pacote shaderc (sudo pacman -S shaderc) ou aponte GLSLC.")
        });
        if !res.status.success() {
            panic!("glslc falhou em {src} ({defines:?}):\n{}", String::from_utf8_lossy(&res.stderr));
        }
    }
}
