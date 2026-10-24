//! Stress de GPU no estilo FurMark (port do GpuBurn.cs): um torus com pelos renderizado por "shells" (a malha é
//! desenhada N vezes, cada camada inflada ao longo da normal, com pixels descartados para formar os fios).
//! Overdraw massivo + cadeia de FMAs no pixel shader + alpha blending, sem limite de FPS.
//! Para sozinho no tempo pedido ou quando a GPU chega ao limite de temperatura.

use crate::ctx::{self, Bind, Ctx, Gfx};
use crate::frames::{Looper, Src, IN_FLIGHT};
use crate::out::{self, Meter, Sensors};
use crate::window::Win;
use crate::Opts;
use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::camera::rh::{proj, view::look_at_mat4};
use glam::{Mat4, Vec3, Vec4};
use serde_json::json;
use std::f32::consts::{FRAC_PI_4, TAU};
use std::time::Instant;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Ubo {
    world: Mat4,
    view_proj: Mat4,
    /// tempo, camadas, comprimento do pelo, iterações de ALU
    p: Vec4,
}

/// (camadas de pelo, iterações de ALU por pixel) de cada intensidade, iguais às do Windows.
fn intensity(name: &str) -> (u32, u32) {
    match name {
        "normal" => (96, 32),
        "extrema" => (160, 64),
        _ => (128, 48),
    }
}

fn build_torus() -> (Vec<Vertex>, Vec<u32>) {
    const SEG: u32 = 160;
    const SIDES: u32 = 64;
    const R: f32 = 1.0;
    const RT: f32 = 0.42;
    let mut v = Vec::with_capacity(((SEG + 1) * (SIDES + 1)) as usize);
    for i in 0..=SEG {
        let u = i as f32 / SEG as f32 * TAU;
        for j in 0..=SIDES {
            let a = j as f32 / SIDES as f32 * TAU;
            let n = Vec3::new(u.cos() * a.cos(), a.sin(), u.sin() * a.cos());
            let p = Vec3::new(u.cos() * R, 0.0, u.sin() * R) + n * RT;
            v.push(Vertex { pos: p.into(), nrm: n.into(), uv: [i as f32 / SEG as f32, j as f32 / SIDES as f32] });
        }
    }
    let mut idx = Vec::with_capacity((SEG * SIDES * 6) as usize);
    for i in 0..SEG {
        for j in 0..SIDES {
            let a = i * (SIDES + 1) + j;
            let b = (i + 1) * (SIDES + 1) + j;
            idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (v, idx)
}

pub fn run(o: &Opts) -> Result<()> {
    let (w, h) = (o.width, o.height);
    let (shells, iterations) = intensity(&o.intensity);
    let mut win = Win::open("RESET EXPLODE · GPU burn", w, h, o.fullscreen)?;
    let ctx = Ctx::new(win.window(), false)?;
    out::info(&ctx.adapter);
    let sensors = Sensors::start(&ctx.adapter.name);
    let d = &ctx.device;

    // MSAA: o maior que a placa aceita até o pedido (8x → 4x → 2x → sem).
    let lim = &ctx.limits;
    let supported = lim.framebuffer_color_sample_counts & lim.framebuffer_depth_sample_counts;
    let mut msaa = o.msaa.max(1);
    while msaa > 1 && !supported.contains(vk::SampleCountFlags::from_raw(msaa)) {
        msaa /= 2;
    }
    let samples = vk::SampleCountFlags::from_raw(msaa);

    // ---- Alvos: cor (com MSAA) + profundidade; com MSAA, uma imagem resolvida para a janela ----
    use vk::ImageUsageFlags as U;
    let fmt = vk::Format::R8G8B8A8_UNORM;
    let depth_fmt = vk::Format::D32_SFLOAT;
    let color_aspect = vk::ImageAspectFlags::COLOR;
    let color = ctx.image(w, h, fmt, U::COLOR_ATTACHMENT | U::TRANSFER_SRC, samples, color_aspect)?;
    let depth = ctx.image(w, h, depth_fmt, U::DEPTH_STENCIL_ATTACHMENT, samples, vk::ImageAspectFlags::DEPTH)?;
    let resolved = if msaa > 1 {
        Some(ctx.image(w, h, fmt, U::TRANSFER_DST | U::TRANSFER_SRC, vk::SampleCountFlags::TYPE_1, color_aspect)?)
    } else {
        None
    };

    // ---- Malha, constantes, pipeline ----
    use vk::BufferUsageFlags as B;
    let (verts, indices) = build_torus();
    let vb = ctx.upload(&verts, B::VERTEX_BUFFER)?;
    let ib = ctx.upload(&indices, B::INDEX_BUFFER)?;
    let ubos = (0..IN_FLIGHT)
        .map(|_| ctx.buffer(std::mem::size_of::<Ubo>() as u64, B::UNIFORM_BUFFER, true))
        .collect::<Result<Vec<_>>>()?;
    let bindings = [(0, vk::DescriptorType::UNIFORM_BUFFER, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
    let dsl = ctx.set_layout(&bindings)?;
    let sets = ctx.alloc_sets(dsl, &bindings, IN_FLIGHT as u32)?;
    for (set, ubo) in sets.iter().zip(&ubos) {
        ctx.write_set(*set, &[(0, Bind::Ubo(ubo))]);
    }
    let layout = ctx.pipeline_layout(&[dsl])?;
    let pipeline = ctx.graphics(
        layout,
        &Gfx {
            vs: spv!("burn.vert"),
            fs: Some(spv!("burn.frag")),
            stride: std::mem::size_of::<Vertex>() as u32,
            attrs: &[
                (0, vk::Format::R32G32B32_SFLOAT, 0),
                (1, vk::Format::R32G32B32_SFLOAT, 12),
                (2, vk::Format::R32G32_SFLOAT, 24),
            ],
            color: Some(fmt),
            depth: Some(depth_fmt),
            depth_test: true,
            depth_write: true,
            // Blending nas camadas: soma carga de ROP e banda de memória (como a pelagem translúcida do FurMark).
            blend: true,
            samples: Some(samples),
            ..Default::default()
        },
    )?;

    let view = look_at_mat4(Vec3::new(0.0, 0.0, -3.5), Vec3::ZERO, Vec3::Y);
    let proj = proj::vulkan::perspective(FRAC_PI_4, w as f32 / h as f32, 0.1, 50.0);
    let color_out = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
    let xfer = vk::PipelineStageFlags2::ALL_TRANSFER;
    let tests = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;

    let mut looper = Looper::new(&ctx, win.size(), 60.0)?;
    let mut meter = Meter::default();
    let clock = Instant::now();
    let stopped;
    let msaa_txt = if msaa > 1 { format!(" · MSAA {msaa}x") } else { String::new() };

    loop {
        win.pump();
        let now = clock.elapsed().as_secs_f64();
        if win.stop_requested() {
            stopped = "usuario";
            break;
        }
        if now >= o.seconds {
            stopped = "tempo";
            break;
        }
        if sensors.read().temp_c.is_some_and(|t| t >= o.temp_limit) {
            stopped = "temperatura";
            break;
        }
        let resized = win.take_resized();
        let f = looper.begin(resized, win.size())?;
        if let Some(ms) = f.gpu_ms {
            meter.push(ms);
        }
        let t = now as f32;
        let world = Mat4::from_rotation_x(0.55 + 0.25 * (t * 0.35).sin()) * Mat4::from_rotation_y(t * 0.6);
        ubos[f.slot].write(&[Ubo {
            world,
            view_proj: proj * view,
            p: Vec4::new(t, shells as f32, 0.2, iterations as f32),
        }]);

        let cmd = f.cmd;
        unsafe {
            ctx::image_barrier(
                d,
                cmd,
                color.image,
                color_aspect,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                (color_out | xfer, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE),
                (color_out, vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE),
            );
            ctx::image_barrier(
                d,
                cmd,
                depth.image,
                vk::ImageAspectFlags::DEPTH,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
                (tests, vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE),
                (tests, vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE),
            );
            ctx::begin_rendering(
                d,
                cmd,
                w,
                h,
                Some(&ctx::color_att(color.view, Some([0.0, 0.0, 0.0, 1.0]))),
                Some(&ctx::depth_att(depth.view, false)),
            );
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, layout, 0, &[sets[f.slot]], &[]);
            d.cmd_bind_vertex_buffers(cmd, 0, &[vb.buf], &[0]);
            d.cmd_bind_index_buffer(cmd, ib.buf, 0, vk::IndexType::UINT32);
            d.cmd_draw_indexed(cmd, indices.len() as u32, shells, 0, 0, 0);
            d.cmd_end_rendering(cmd);
        }

        // Com MSAA, resolve só nos frames que vão para a janela (como o ResolveSubresource do Windows).
        let src = match &resolved {
            Some(r) if looper.present_due() => {
                ctx::image_barrier(
                    d,
                    f.cmd,
                    color.image,
                    color_aspect,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    (color_out, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE),
                    (xfer, vk::AccessFlags2::TRANSFER_READ),
                );
                ctx::image_barrier(
                    d,
                    f.cmd,
                    r.image,
                    color_aspect,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    (xfer, vk::AccessFlags2::NONE),
                    (xfer, vk::AccessFlags2::TRANSFER_WRITE),
                );
                let layers = vk::ImageSubresourceLayers { aspect_mask: color_aspect, mip_level: 0, base_array_layer: 0, layer_count: 1 };
                let region = vk::ImageResolve {
                    src_subresource: layers,
                    src_offset: vk::Offset3D::default(),
                    dst_subresource: layers,
                    dst_offset: vk::Offset3D::default(),
                    extent: vk::Extent3D { width: w, height: h, depth: 1 },
                };
                unsafe {
                    d.cmd_resolve_image(
                        f.cmd,
                        color.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        r.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[region],
                    )
                };
                Src {
                    image: r.image,
                    layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    stage: xfer,
                    access: vk::AccessFlags2::TRANSFER_WRITE,
                    w,
                    h,
                }
            }
            _ => Src {
                image: color.image,
                layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                stage: color_out,
                access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                w,
                h,
            },
        };
        looper.end(f, &src)?;

        if meter.tick(now) {
            let s = sensors.read();
            out::progress(now, meter.fps, s, json!({}));
            let temp = s.temp_c.map(|t| format!(" · {t:.0} °C")).unwrap_or_default();
            win.set_title(&format!(
                "RESET EXPLODE · GPU burn · {:.0} FPS · {w}x{h}{msaa_txt}{temp} · {}",
                meter.fps, ctx.adapter.name
            ));
        }
    }
    looper.idle();
    let s = sensors.read();
    out::result(
        "burn",
        meter.summary(),
        clock.elapsed().as_secs_f64(),
        None,
        stopped,
        json!({
            "width": w,
            "height": h,
            "msaa": if msaa > 1 { msaa } else { 0 },
            "intensity": o.intensity,
            "temp_c": s.temp_c.map(|t| t.round() as i64),
            "max_temp_c": s.max_temp_c.map(|t| t.round() as i64),
            "temp_limit": o.temp_limit,
        }),
    );
    Ok(())
}
