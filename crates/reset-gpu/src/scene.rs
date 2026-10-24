//! Cena de jogo (port do GpuGameBench.cs, no estilo 3DMark/Unigine): vale com terreno por ruído fractal, 40 mil
//! árvores por instancing, sombras em tempo real (shadow map 4096² com PCF 5x5), água com ondas procedurais, neblina,
//! céu e pós-processamento HDR. Câmera em trajeto fixo pelo tempo e resolução fixa: o trabalho é o mesmo em qualquer
//! PC. Nota = FPS médio × 10.

use crate::ctx::{self, Bind, Ctx, Gfx};
use crate::frames::{Looper, Src, IN_FLIGHT};
use crate::out::{self, Meter, Sensors};
use crate::window::Win;
use crate::Opts;
use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::camera::rh::{proj, view::look_at_mat4};
use glam::{Mat4, Vec2, Vec3, Vec4};
use serde_json::json;
use std::time::Instant;

const TERRAIN_N: usize = 512; // vértices por lado
const TERRAIN_SIZE: f32 = 800.0;
const WATER_Y: f32 = 6.0;
const TREE_COUNT: usize = 40_000;
const SHADOW_RES: u32 = 4096;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    /// 0 = terreno, 1 = tronco, 2 = folhas
    mat: f32,
}

/// Mesmo layout do bloco `Frame` de shaders/scene_common.glsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUbo {
    view_proj: Mat4,
    light_view_proj: Mat4,
    inv_view_proj: Mat4,
    cam_time: Vec4,
    sun_dir: Vec4,
}

// ------------------------------------------------------------------ cena (CPU)

/// Altura do terreno: ruído fractal (fBm) com um vale sinuoso cortando o mapa.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    let (mut h, mut amp, mut freq) = (0.0, 1.0, 0.005);
    for o in 0..6 {
        let o = o as f32;
        h += amp * value_noise(x * freq + o * 17.3, z * freq - o * 9.1);
        amp *= 0.5;
        freq *= 2.03;
    }
    h = h * 55.0 + 10.0;
    let river = (z * 0.007).sin() * 120.0; // curva do vale
    let valley = ((x - river).abs() / 90.0).min(1.0);
    h * (0.25 + 0.75 * valley * valley) - (1.0 - valley) * 4.0
}

fn value_noise(x: f32, z: f32) -> f32 {
    let (xi, zi) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - xi as f32, z - zi as f32);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let (a, b, c, d) = (hash(xi, zi), hash(xi + 1, zi), hash(xi, zi + 1), hash(xi + 1, zi + 1));
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

fn hash(x: i32, z: i32) -> f32 {
    let h = x.wrapping_mul(374761393).wrapping_add(z.wrapping_mul(668265263)) as u32;
    let h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xFFFFFF) as f32 / 16777215.0
}

fn build_terrain() -> (Vec<Vertex>, Vec<u32>) {
    let n = TERRAIN_N;
    let step = TERRAIN_SIZE / (n - 1) as f32;
    let half = TERRAIN_SIZE / 2.0;
    let mut v = Vec::with_capacity(n * n);
    for z in 0..n {
        for x in 0..n {
            let (wx, wz) = (x as f32 * step - half, z as f32 * step - half);
            let h = terrain_height(wx, wz);
            let hx = terrain_height(wx + 0.5, wz) - terrain_height(wx - 0.5, wz);
            let hz = terrain_height(wx, wz + 0.5) - terrain_height(wx, wz - 0.5);
            let nrm = Vec3::new(-hx, 1.0, -hz).normalize();
            v.push(Vertex { pos: [wx, h, wz], nrm: nrm.into(), mat: 0.0 });
        }
    }
    let mut idx = Vec::with_capacity((n - 1) * (n - 1) * 6);
    for z in 0..n - 1 {
        for x in 0..n - 1 {
            let a = (z * n + x) as u32;
            let (b, c) = (a + 1, a + n as u32);
            let d = c + 1;
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    (v, idx)
}

/// Árvore low-poly: tronco (cilindro) + 3 cones de folhagem. Mat 1 = tronco, 2 = folhas.
fn build_tree() -> (Vec<Vertex>, Vec<u32>) {
    let mut v = Vec::new();
    let mut idx = Vec::new();
    let mut cone = |base_y: f32, height: f32, r0: f32, r1: f32, seg: u32, mat: f32| {
        let start = v.len() as u32;
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * std::f32::consts::TAU;
            let (c, s) = (a.cos(), a.sin());
            let slope = (r0 - r1) / height;
            let n: [f32; 3] = Vec3::new(c, slope, s).normalize().into();
            v.push(Vertex { pos: [c * r0, base_y, s * r0], nrm: n, mat });
            v.push(Vertex { pos: [c * r1, base_y + height, s * r1], nrm: n, mat });
        }
        for i in 0..seg {
            let a = start + i * 2;
            idx.extend_from_slice(&[a, a + 1, a + 2, a + 2, a + 1, a + 3]);
        }
    };
    cone(0.0, 2.0, 0.25, 0.2, 8, 1.0);
    cone(1.4, 2.6, 2.0, 0.05, 12, 2.0);
    cone(2.8, 2.3, 1.6, 0.05, 12, 2.0);
    cone(4.0, 2.0, 1.1, 0.02, 12, 2.0);
    (v, idx)
}

/// Posições das árvores (semente fixa): fora da água, abaixo dos picos e longe das encostas íngremes.
fn place_trees() -> Vec<Vec4> {
    let mut list = Vec::with_capacity(TREE_COUNT);
    let mut s: u32 = 2024;
    let mut r = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / 16777216.0
    };
    while list.len() < TREE_COUNT {
        let x = (r() - 0.5) * TERRAIN_SIZE * 0.98;
        let z = (r() - 0.5) * TERRAIN_SIZE * 0.98;
        let h = terrain_height(x, z);
        let slope = (terrain_height(x + 1.0, z) - terrain_height(x - 1.0, z)).abs()
            + (terrain_height(x, z + 1.0) - terrain_height(x, z - 1.0)).abs();
        if h < WATER_Y + 1.5 || h > 48.0 || slope > 1.4 {
            continue;
        }
        list.push(Vec4::new(x, h - 0.2, z, 0.7 + r() * 0.9));
    }
    list
}

/// Trajeto da câmera (Catmull-Rom fechado): sobe o vale por uma margem do rio e volta pela outra.
fn camera_path() -> Vec<Vec2> {
    let mut p = Vec::new();
    for i in 0..=6 {
        let z = -330.0 + i as f32 * 110.0;
        p.push(Vec2::new((z * 0.007).sin() * 120.0 + 45.0, z));
    }
    for i in (0..=6).rev() {
        let z = -330.0 + i as f32 * 110.0;
        p.push(Vec2::new((z * 0.007).sin() * 120.0 - 75.0, z));
    }
    p
}

fn path_at(path: &[Vec2], t: f32) -> Vec2 {
    let n = path.len();
    let f = t.rem_euclid(1.0) * n as f32;
    let i = (f as usize).min(n - 1);
    let u = f - i as f32;
    let (p0, p1, p2, p3) = (path[(i + n - 1) % n], path[i], path[(i + 1) % n], path[(i + 2) % n]);
    0.5 * (2.0 * p1 + (-p0 + p2) * u + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u * u + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * u * u * u)
}

fn cam_height(p: Vec2) -> f32 {
    let mut h = WATER_Y;
    for dz in -2..=2 {
        for dx in -2..=2 {
            h = h.max(terrain_height(p.x + dx as f32 * 10.0, p.y + dz as f32 * 10.0));
        }
    }
    h + 26.0
}

// ------------------------------------------------------------------ execução

pub fn run(o: &Opts) -> Result<()> {
    let (w, h) = (o.width, o.height);
    let mut win = Win::open("RESET BENCH · Cena de jogo", w, h, o.fullscreen)?;
    let ctx = Ctx::new(win.window(), false)?;
    out::info(&ctx.adapter);
    let sensors = Sensors::start(&ctx.adapter.name);
    let d = &ctx.device;

    // ---- Alvos: HDR + profundidade, LDR, mapa de sombras ----
    use vk::ImageUsageFlags as U;
    let one = vk::SampleCountFlags::TYPE_1;
    let (color, depth_aspect) = (vk::ImageAspectFlags::COLOR, vk::ImageAspectFlags::DEPTH);
    let hdr_fmt = vk::Format::R16G16B16A16_SFLOAT;
    let ldr_fmt = vk::Format::R8G8B8A8_UNORM;
    let depth_fmt = vk::Format::D32_SFLOAT;
    let hdr = ctx.image(w, h, hdr_fmt, U::COLOR_ATTACHMENT | U::SAMPLED, one, color)?;
    let depth = ctx.image(w, h, depth_fmt, U::DEPTH_STENCIL_ATTACHMENT, one, depth_aspect)?;
    let ldr = ctx.image(w, h, ldr_fmt, U::COLOR_ATTACHMENT | U::TRANSFER_SRC, one, color)?;
    let shadow = ctx.image(SHADOW_RES, SHADOW_RES, depth_fmt, U::DEPTH_STENCIL_ATTACHMENT | U::SAMPLED, one, depth_aspect)?;

    // ---- Geometria ----
    use vk::BufferUsageFlags as B;
    let (tv, ti) = build_terrain();
    let terrain_vb = ctx.upload(&tv, B::VERTEX_BUFFER)?;
    let terrain_ib = ctx.upload(&ti, B::INDEX_BUFFER)?;
    let (trv, tri) = build_tree();
    let tree_vb = ctx.upload(&trv, B::VERTEX_BUFFER)?;
    let tree_ib = ctx.upload(&tri, B::INDEX_BUFFER)?;
    let trees = place_trees();
    let tree_buf = ctx.upload(&trees, B::STORAGE_BUFFER)?;
    let wh = TERRAIN_SIZE / 2.0;
    let wv = |x: f32, z: f32| Vertex { pos: [x, WATER_Y, z], nrm: [0.0, 1.0, 0.0], mat: 0.0 };
    let water_vb = ctx.upload(&[wv(-wh, -wh), wv(wh, -wh), wv(-wh, wh), wv(wh, wh)], B::VERTEX_BUFFER)?;
    let water_ib = ctx.upload(&[0u32, 2, 1, 1, 2, 3], B::INDEX_BUFFER)?;
    let ubos = (0..IN_FLIGHT)
        .map(|_| ctx.buffer(std::mem::size_of::<FrameUbo>() as u64, B::UNIFORM_BUFFER, true))
        .collect::<Result<Vec<_>>>()?;

    // ---- Amostradores e descritores ----
    let clamp = vk::SamplerAddressMode::CLAMP_TO_EDGE;
    let base = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(clamp)
        .address_mode_v(clamp)
        .address_mode_w(clamp)
        .max_lod(0.0);
    let cmp_sampler = ctx.sampler(&base.compare_enable(true).compare_op(vk::CompareOp::LESS_OR_EQUAL))?;
    let lin_sampler = ctx.sampler(&base)?;
    use vk::DescriptorType as T;
    use vk::ShaderStageFlags as S;
    let bindings = [
        (0, T::UNIFORM_BUFFER, S::VERTEX | S::FRAGMENT),
        (1, T::STORAGE_BUFFER, S::VERTEX),
        (2, T::COMBINED_IMAGE_SAMPLER, S::FRAGMENT),
        (3, T::COMBINED_IMAGE_SAMPLER, S::FRAGMENT),
    ];
    let dsl = ctx.set_layout(&bindings)?;
    let sets = ctx.alloc_sets(dsl, &bindings, IN_FLIGHT as u32)?;
    for (set, ubo) in sets.iter().zip(&ubos) {
        ctx.write_set(
            *set,
            &[
                (0, Bind::Ubo(ubo)),
                (1, Bind::Ssbo(&tree_buf)),
                (2, Bind::Tex(shadow.view, cmp_sampler)),
                (3, Bind::Tex(hdr.view, lin_sampler)),
            ],
        );
    }
    let layout = ctx.pipeline_layout(&[dsl])?;

    // ---- Pipelines ----
    let attrs = [
        (0, vk::Format::R32G32B32_SFLOAT, 0),
        (1, vk::Format::R32G32B32_SFLOAT, 12),
        (2, vk::Format::R32_SFLOAT, 24),
    ];
    let stride = std::mem::size_of::<Vertex>() as u32;
    let shadow_pipe = |vs: &[u8]| {
        ctx.graphics(
            layout,
            &Gfx {
                vs,
                stride,
                attrs: &attrs,
                depth: Some(depth_fmt),
                depth_test: true,
                depth_write: true,
                bias: Some((800.0, 2.0)),
                ..Default::default()
            },
        )
    };
    let shadow_terrain = shadow_pipe(spv!("scene_shadow_terrain.vert"))?;
    let shadow_tree = shadow_pipe(spv!("scene_shadow_tree.vert"))?;
    let lit = |vs: &[u8], fs: &[u8]| {
        ctx.graphics(
            layout,
            &Gfx {
                vs,
                fs: Some(fs),
                stride,
                attrs: &attrs,
                color: Some(hdr_fmt),
                depth: Some(depth_fmt),
                depth_test: true,
                depth_write: true,
                ..Default::default()
            },
        )
    };
    let terrain_pipe = lit(spv!("scene_terrain.vert"), spv!("scene_terrain.frag"))?;
    let tree_pipe = lit(spv!("scene_tree.vert"), spv!("scene_tree.frag"))?;
    let water_pipe = lit(spv!("scene_water.vert"), spv!("scene_water.frag"))?;
    let sky_pipe = ctx.graphics(
        layout,
        &Gfx {
            vs: spv!("fullscreen.vert"),
            fs: Some(spv!("scene_sky.frag")),
            color: Some(hdr_fmt),
            depth: Some(depth_fmt),
            ..Default::default()
        },
    )?;
    let post_pipe = ctx.graphics(
        layout,
        &Gfx { vs: spv!("fullscreen.vert"), fs: Some(spv!("scene_post.frag")), color: Some(ldr_fmt), ..Default::default() },
    )?;

    // ---- Luz e câmera ----
    let sun = Vec3::new(-0.45, -0.55, -0.35).normalize();
    let light_vp = proj::directx::orthographic(-450.0, 450.0, -450.0, 450.0, 1.0, 1900.0)
        * look_at_mat4(-sun * 900.0, Vec3::ZERO, Vec3::Y);
    let proj = proj::vulkan::perspective(std::f32::consts::FRAC_PI_3, w as f32 / h as f32, 0.5, 2000.0);
    let path = camera_path();

    let mut looper = Looper::new(&ctx, win.size(), 60.0)?;
    let mut meter = Meter::default();
    let duration = o.seconds;
    let clock = Instant::now();
    let stopped;
    let src = Src {
        image: ldr.image,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        w,
        h,
    };
    let stage_frag = vk::PipelineStageFlags2::FRAGMENT_SHADER;
    let tests = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
    let color_out = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
    let depth_rw = vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;
    let color_rw = vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE;

    loop {
        win.pump();
        let now = clock.elapsed().as_secs_f64();
        if win.stop_requested() {
            stopped = "usuario";
            break;
        }
        if now >= duration {
            stopped = "tempo";
            break;
        }
        let resized = win.take_resized();
        let f = looper.begin(resized, win.size())?;
        if let Some(ms) = f.gpu_ms {
            meter.push(ms);
        }

        // Câmera pelo tempo (não pelos frames): a mesma cena aparece no mesmo segundo em qualquer PC.
        let t = (now / duration) as f32;
        let (p, ahead) = (path_at(&path, t), path_at(&path, t + 0.035));
        let eye = Vec3::new(p.x, cam_height(p), p.y);
        let target = Vec3::new(ahead.x, cam_height(ahead) - 22.0, ahead.y);
        let vp = proj * look_at_mat4(eye, target, Vec3::Y);
        ubos[f.slot].write(&[FrameUbo {
            view_proj: vp,
            light_view_proj: light_vp,
            inv_view_proj: vp.inverse(),
            cam_time: eye.extend(now as f32),
            sun_dir: sun.extend(0.0),
        }]);

        let cmd = f.cmd;
        unsafe {
            d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, layout, 0, &[sets[f.slot]], &[]);

            // 1) Mapa de sombras
            ctx::image_barrier(
                d,
                cmd,
                shadow.image,
                depth_aspect,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
                (stage_frag | tests, vk::AccessFlags2::NONE),
                (tests, depth_rw),
            );
            ctx::begin_rendering(d, cmd, SHADOW_RES, SHADOW_RES, None, Some(&ctx::depth_att(shadow.view, true)));
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, shadow_terrain);
            d.cmd_bind_vertex_buffers(cmd, 0, &[terrain_vb.buf], &[0]);
            d.cmd_bind_index_buffer(cmd, terrain_ib.buf, 0, vk::IndexType::UINT32);
            d.cmd_draw_indexed(cmd, ti.len() as u32, 1, 0, 0, 0);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, shadow_tree);
            d.cmd_bind_vertex_buffers(cmd, 0, &[tree_vb.buf], &[0]);
            d.cmd_bind_index_buffer(cmd, tree_ib.buf, 0, vk::IndexType::UINT32);
            d.cmd_draw_indexed(cmd, tri.len() as u32, TREE_COUNT as u32, 0, 0, 0);
            d.cmd_end_rendering(cmd);

            // 2) Cena em HDR: céu, terreno, árvores, água
            ctx::image_barrier(
                d,
                cmd,
                shadow.image,
                depth_aspect,
                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                (tests, vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE),
                (stage_frag, vk::AccessFlags2::SHADER_SAMPLED_READ),
            );
            ctx::image_barrier(
                d,
                cmd,
                hdr.image,
                color,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                (stage_frag | color_out, vk::AccessFlags2::NONE),
                (color_out, color_rw),
            );
            ctx::image_barrier(
                d,
                cmd,
                depth.image,
                depth_aspect,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
                (tests, vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE),
                (tests, depth_rw),
            );
            ctx::begin_rendering(d, cmd, w, h, Some(&ctx::color_att(hdr.view, None)), Some(&ctx::depth_att(depth.view, false)));
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, sky_pipe);
            d.cmd_draw(cmd, 3, 1, 0, 0);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, terrain_pipe);
            d.cmd_bind_vertex_buffers(cmd, 0, &[terrain_vb.buf], &[0]);
            d.cmd_bind_index_buffer(cmd, terrain_ib.buf, 0, vk::IndexType::UINT32);
            d.cmd_draw_indexed(cmd, ti.len() as u32, 1, 0, 0, 0);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, tree_pipe);
            d.cmd_bind_vertex_buffers(cmd, 0, &[tree_vb.buf], &[0]);
            d.cmd_bind_index_buffer(cmd, tree_ib.buf, 0, vk::IndexType::UINT32);
            d.cmd_draw_indexed(cmd, tri.len() as u32, TREE_COUNT as u32, 0, 0, 0);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, water_pipe);
            d.cmd_bind_vertex_buffers(cmd, 0, &[water_vb.buf], &[0]);
            d.cmd_bind_index_buffer(cmd, water_ib.buf, 0, vk::IndexType::UINT32);
            d.cmd_draw_indexed(cmd, 6, 1, 0, 0, 0);
            d.cmd_end_rendering(cmd);

            // 3) Pós-processamento: HDR → LDR (tone mapping + vinheta)
            ctx::image_barrier(
                d,
                cmd,
                hdr.image,
                color,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                (color_out, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE),
                (stage_frag, vk::AccessFlags2::SHADER_SAMPLED_READ),
            );
            ctx::image_barrier(
                d,
                cmd,
                ldr.image,
                color,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                (color_out | vk::PipelineStageFlags2::ALL_TRANSFER, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE),
                (color_out, color_rw),
            );
            ctx::begin_rendering(d, cmd, w, h, Some(&ctx::color_att(ldr.view, None)), None);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, post_pipe);
            d.cmd_draw(cmd, 3, 1, 0, 0);
            d.cmd_end_rendering(cmd);
        }
        looper.end(f, &src)?;

        if meter.tick(now) {
            let s = sensors.read();
            out::progress(now, meter.fps, s, json!({}));
            win.set_title(&format!(
                "RESET BENCH · Cena de jogo · {w}x{h} · {:.0} FPS · {:.0}%",
                meter.fps,
                now / duration * 100.0
            ));
        }
    }
    looper.idle();
    let seconds = clock.elapsed().as_secs_f64();
    let sum = meter.summary();
    let s = sensors.read();
    out::result(
        "scene",
        sum,
        seconds,
        Some(sum.avg_fps * 10.0),
        stopped,
        json!({ "width": w, "height": h, "max_temp_c": s.max_temp_c.map(|t| t.round() as i64) }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terreno_e_trajeto() {
        // O vale fica abaixo da água no meio do rio e o trajeto da câmera é fechado (volta ao começo).
        assert!(terrain_height(0.0, 0.0) < 40.0);
        let path = camera_path();
        assert!((path_at(&path, 0.0) - path_at(&path, 1.0)).length() < 1e-3);
        assert_eq!(place_trees().len(), TREE_COUNT);
    }
}
