//! RESET BENCH · Ray tracing (port do GpuRtBench.cs, que no Windows usa DXR 1.1): usa os núcleos de RT da placa
//! pelo Vulkan (VK_KHR_ray_query + VK_KHR_acceleration_structure). Cena procedural com 4 mil objetos (esferas, toros,
//! colunas, caixas) em materiais fosco, metal e espelho, mais luzes emissivas. Por pixel, um path tracer com raios
//! inline num compute shader: visibilidade, sombra suave do sol, oclusão de ambiente e rebatidas de reflexo/iluminação
//! indireta. A estrutura de aceleração do topo (TLAS) é refeita a cada frame com objetos girando, como nos jogos.
//!
//! Para comparar com confiança: aquecimento sem medir, depois 3 passadas iguais; FPS pela mediana das passadas e a
//! variação entre elas. `--seconds` é o tempo total (aquecimento + passadas).
//! Nota = amostras por segundo ÷ 10.000 (amostras = pixels × amostras por pixel × FPS).

use crate::ctx::{Bind, Buffer, Ctx};
use crate::frames::{Looper, Src, IN_FLIGHT};
use crate::out::{self, Meter, Sensors, Summary};
use crate::window::Win;
use crate::Opts;
use anyhow::{Context, Result};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::camera::rh::{proj, view::look_at_mat4};
use glam::{Mat4, Vec2, Vec3, Vec4};
use serde_json::json;
use std::f32::consts::{PI, TAU};
use std::time::Instant;

const OBJECT_COUNT: usize = 4000;
const WORLD: f32 = 220.0;
const MESH_GROUND: u32 = 0;
const MESH_SPHERE: u32 = 1;
const MESH_HERO_SPHERE: u32 = 2;
const MESH_TORUS: u32 = 3;
const MESH_COLUMN: u32 = 4;
const MESH_BOX: u32 = 5;

/// (amostras por pixel, rebatidas) de cada preset, iguais aos do Windows.
fn quality(preset: &str) -> (u32, u32) {
    match preset {
        "normal" => (2, 2),
        "extrema" => (8, 4),
        _ => (4, 3),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vtx {
    pos: [f32; 3],
    _p0: f32,
    nrm: [f32; 3],
    _p1: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MeshInfo {
    index_offset: u32,
    vertex_offset: u32,
    _pad: [u32; 2],
}

/// Material por objeto (mesmo layout do `Inst` de shaders/rt.comp).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct InstMat {
    albedo: [f32; 3],
    metal: f32,
    rough: f32,
    emissive: f32,
    mesh: u32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUbo {
    inv_view_proj: Mat4,
    cam_time: Vec4,
    sun_dir: Vec4,
    /// índice do frame, largura, altura, amostras por pixel
    p: [u32; 4],
    /// rebatidas
    p2: [u32; 4],
}

// ------------------------------------------------------------------ malhas

#[derive(Default)]
struct MeshBuilder {
    v: Vec<Vtx>,
    i: Vec<u32>,
    /// (primeiro índice, quantos índices, primeiro vértice, quantos vértices)
    meshes: Vec<(u32, u32, u32, u32)>,
    v_start: usize,
    i_start: usize,
}

impl MeshBuilder {
    fn begin(&mut self) {
        self.v_start = self.v.len();
        self.i_start = self.i.len();
    }
    fn end(&mut self) {
        self.meshes.push((
            self.i_start as u32,
            (self.i.len() - self.i_start) as u32,
            self.v_start as u32,
            (self.v.len() - self.v_start) as u32,
        ));
    }
    fn add(&mut self, p: Vec3, n: Vec3) -> u32 {
        self.v.push(Vtx { pos: p.into(), _p0: 0.0, nrm: n.normalize().into(), _p1: 0.0 });
        (self.v.len() - 1 - self.v_start) as u32
    }
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.i.extend_from_slice(&[a, b, c]);
    }

    fn plane(&mut self, size: f32, n: u32) {
        self.begin();
        for z in 0..=n {
            for x in 0..=n {
                let p = Vec3::new((x as f32 / n as f32 - 0.5) * size, 0.0, (z as f32 / n as f32 - 0.5) * size);
                self.add(p, Vec3::Y);
            }
        }
        for z in 0..n {
            for x in 0..n {
                let a = z * (n + 1) + x;
                let c = a + n + 1;
                self.tri(a, c, a + 1);
                self.tri(a + 1, c, c + 1);
            }
        }
        self.end();
    }

    fn sphere(&mut self, seg: u32) {
        self.begin();
        let rings = seg / 2;
        for r in 0..=rings {
            for s in 0..=seg {
                let (th, ph) = (PI * r as f32 / rings as f32, TAU * s as f32 / seg as f32);
                let p = Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
                self.add(p, p);
            }
        }
        for r in 0..rings {
            for s in 0..seg {
                let a = r * (seg + 1) + s;
                let c = a + seg + 1;
                self.tri(a, a + 1, c);
                self.tri(a + 1, c + 1, c);
            }
        }
        self.end();
    }

    fn torus(&mut self, seg: u32, side: u32, big_r: f32, r: f32) {
        self.begin();
        for i in 0..=seg {
            for j in 0..=side {
                let (u, v) = (TAU * i as f32 / seg as f32, TAU * j as f32 / side as f32);
                let c = Vec3::new(u.cos() * big_r, 0.0, u.sin() * big_r);
                let n = Vec3::new(u.cos() * v.cos(), v.sin(), u.sin() * v.cos());
                self.add(c + n * r, n);
            }
        }
        for i in 0..seg {
            for j in 0..side {
                let a = i * (side + 1) + j;
                let c = a + side + 1;
                self.tri(a, c, a + 1);
                self.tri(a + 1, c, c + 1);
            }
        }
        self.end();
    }

    /// Coluna com caneluras: o raio ondula como numa coluna grega (mais triângulos e normais variadas).
    fn column(&mut self, seg: u32) {
        self.begin();
        for i in 0..=seg {
            let a = TAU * i as f32 / seg as f32;
            let rr = 1.0 + 0.06 * (a * 12.0).cos();
            let n = Vec3::new(a.cos(), 0.0, a.sin());
            self.add(Vec3::new(a.cos() * rr, 0.0, a.sin() * rr), n);
            self.add(Vec3::new(a.cos() * rr, 1.0, a.sin() * rr), n);
        }
        for i in 0..seg {
            let a = i * 2;
            self.tri(a, a + 1, a + 2);
            self.tri(a + 1, a + 3, a + 2);
        }
        let top = self.add(Vec3::Y, Vec3::Y);
        for i in 0..seg {
            let (a0, a1) = (TAU * i as f32 / seg as f32, TAU * (i + 1) as f32 / seg as f32);
            let p0 = self.add(Vec3::new(a0.cos(), 1.0, a0.sin()), Vec3::Y);
            let p1 = self.add(Vec3::new(a1.cos(), 1.0, a1.sin()), Vec3::Y);
            self.tri(top, p1, p0);
        }
        self.end();
    }

    fn cube(&mut self) {
        self.begin();
        for f in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            let u = if f.y.abs() > 0.5 { Vec3::X } else { Vec3::Y };
            let v = f.cross(u);
            let a = self.add(f - u - v, f);
            let c = self.add(f + u - v, f);
            let d = self.add(f + u + v, f);
            let e = self.add(f - u + v, f);
            self.tri(a, c, d);
            self.tri(a, d, e);
        }
        self.end();
    }
}

// ------------------------------------------------------------------ objetos

struct Obj {
    mesh: u32,
    pos: Vec3,
    scale: Vec3,
    spin: f32,
    axis: Vec3,
    mat: InstMat,
}

/// Gerador fixo (xorshift): a mesma cena em qualquer PC.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 16777216.0
    }
}

/// Cores saturadas de uma paleta fixa, com brilho variado.
fn palette(pick: f32, shade: f32) -> [f32; 3] {
    const P: [[f32; 3]; 8] = [
        [0.85, 0.18, 0.12],
        [0.95, 0.55, 0.08],
        [0.12, 0.45, 0.85],
        [0.1, 0.65, 0.4],
        [0.9, 0.85, 0.75],
        [0.45, 0.2, 0.7],
        [0.08, 0.08, 0.1],
        [0.75, 0.75, 0.78],
    ];
    let c = P[(pick * P.len() as f32) as usize % P.len()];
    let k = 0.55 + shade * 0.45;
    [c[0] * k, c[1] * k, c[2] * k]
}

fn build_objects() -> Vec<Obj> {
    let mut rnd = Rng(20261015);
    let mut r = || rnd.next();
    let mut list = vec![Obj {
        mesh: MESH_GROUND,
        pos: Vec3::ZERO,
        scale: Vec3::ONE,
        spin: 0.0,
        axis: Vec3::Y,
        mat: InstMat { albedo: [0.55, 0.55, 0.58], metal: 0.15, rough: 0.35, mesh: MESH_GROUND, ..Default::default() },
    }];
    // Esferas-espelho e douradas grandes no centro (reflexos de reflexos).
    for i in 0..7 {
        let a = i as f32 / 7.0 * TAU;
        let gold = i % 2 == 1;
        list.push(Obj {
            mesh: MESH_HERO_SPHERE,
            pos: Vec3::new(a.cos() * 16.0, 6.0, a.sin() * 16.0),
            scale: Vec3::splat(6.0),
            spin: 0.0,
            axis: Vec3::Y,
            mat: InstMat {
                albedo: if gold { [1.0, 0.78, 0.34] } else { [0.95, 0.95, 0.97] },
                metal: 1.0,
                rough: if gold { 0.12 } else { 0.0 },
                mesh: MESH_HERO_SPHERE,
                ..Default::default()
            },
        });
    }
    while list.len() < OBJECT_COUNT {
        let mut pos = Vec3::new((r() - 0.5) * WORLD, 0.0, (r() - 0.5) * WORLD);
        if Vec2::new(pos.x, pos.z).length() < 26.0 {
            continue; // deixa o centro livre para as esferas grandes
        }
        let mesh = match r() {
            x if x < 0.35 => MESH_SPHERE,
            x if x < 0.55 => MESH_TORUS,
            x if x < 0.75 => MESH_COLUMN,
            _ => MESH_BOX,
        };
        let s = 0.6 + r() * 1.8;
        let scale = if mesh == MESH_COLUMN { Vec3::new(s * 0.6, s * (2.0 + r() * 3.0), s * 0.6) } else { Vec3::splat(s) };
        pos.y = match mesh {
            MESH_SPHERE | MESH_BOX => s,
            MESH_TORUS => s * 0.35 + 0.6,
            _ => 0.0,
        };
        let roll = r();
        let albedo = palette(r(), r());
        let metal = roll < 0.3;
        let rough = if metal { r() * 0.35 } else { 0.6 + r() * 0.4 };
        let emissive = if roll > 0.97 { 6.0 + r() * 10.0 } else { 0.0 };
        let mat = InstMat { albedo, metal: if metal { 1.0 } else { 0.0 }, rough, emissive, mesh, _pad: 0.0 };
        let spin = if matches!(mesh, MESH_TORUS | MESH_BOX) && r() < 0.5 { 0.3 + r() * 1.2 } else { 0.0 };
        let axis = Vec3::new(r() - 0.5, 1.0, r() - 0.5).normalize();
        list.push(Obj { mesh, pos, scale, spin, axis, mat });
    }
    list
}

fn obj_matrix(o: &Obj, t: f32) -> Mat4 {
    let rot = if o.spin != 0.0 { Mat4::from_axis_angle(o.axis, t * o.spin) } else { Mat4::IDENTITY };
    Mat4::from_translation(o.pos) * rot * Mat4::from_scale(o.scale)
}

/// Câmera em trajeto fixo pelo tempo da passada (0..1): mesma sequência de imagens em qualquer placa.
fn camera_at(t: f32) -> (Vec3, Vec3) {
    let a = t * TAU * 0.75 + 0.4;
    let r = 40.0 + 26.0 * (t * PI).sin();
    let eye = Vec3::new(a.cos() * r, 12.0 + 10.0 * (0.5 + 0.5 * (t * TAU * 1.5).sin()), a.sin() * r);
    let target = Vec3::new((a + 0.6).cos() * 6.0, 3.0, (a + 0.6).sin() * 6.0);
    (eye, target)
}

// ------------------------------------------------------------------ estruturas de aceleração

/// Estrutura de aceleração com o buffer que a guarda.
fn make_accel(ctx: &Ctx, ty: vk::AccelerationStructureTypeKHR, size: u64) -> Result<(vk::AccelerationStructureKHR, u64)> {
    let accel = ctx.accel.as_ref().context("contexto sem ray tracing")?;
    let buf = ctx.buffer(
        size,
        vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        false,
    )?;
    let info = vk::AccelerationStructureCreateInfoKHR::default().buffer(buf.buf).size(size).ty(ty);
    let a = unsafe { accel.create_acceleration_structure(&info, None) }.context("falha ao criar estrutura de aceleração")?;
    ctx.keep_accel(a);
    let addr = unsafe {
        accel.get_acceleration_structure_device_address(
            &vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(a),
        )
    };
    Ok((a, addr))
}

/// Área de rascunho da montagem, com o endereço alinhado como o driver pede.
fn scratch(ctx: &Ctx, size: u64) -> Result<(Buffer, u64)> {
    let align = ctx.scratch_align;
    let buf = ctx.buffer(
        size + align,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        false,
    )?;
    let addr = buf.addr.div_ceil(align) * align;
    Ok((buf, addr))
}

// ------------------------------------------------------------------ execução

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

pub fn run(o: &Opts) -> Result<()> {
    let (w, h) = (o.width, o.height);
    let preset = o.preset.as_str();
    let (spp, bounces) = quality(preset);
    let mut win = Win::open("RESET BENCH · Ray tracing", w, h, o.fullscreen)?;
    let ctx = Ctx::new(win.window(), true)?;
    out::info(&ctx.adapter);
    let accel = ctx.accel.clone().context("contexto sem ray tracing")?;
    let sensors = Sensors::start(&ctx.adapter.name);
    let d = &ctx.device;

    // ---- Geometria ----
    let mut mb = MeshBuilder::default();
    mb.plane(WORLD + 60.0, 96);
    mb.sphere(32);
    mb.sphere(160); // esfera grande, ~25 mil triângulos
    mb.torus(48, 24, 1.0, 0.35);
    mb.column(48);
    mb.cube();
    use vk::BufferUsageFlags as B;
    let geo_usage = B::STORAGE_BUFFER | B::SHADER_DEVICE_ADDRESS | B::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR;
    let vb = ctx.upload(&mb.v, geo_usage)?;
    let ib = ctx.upload(&mb.i, geo_usage)?;
    let mesh_info: Vec<MeshInfo> = mb
        .meshes
        .iter()
        .map(|&(io, _, vo, _)| MeshInfo { index_offset: io, vertex_offset: vo, _pad: [0; 2] })
        .collect();
    let meshes_buf = ctx.upload(&mesh_info, B::STORAGE_BUFFER)?;
    let objects = build_objects();
    let mats: Vec<InstMat> = objects.iter().map(|o| o.mat).collect();
    let mats_buf = ctx.upload(&mats, B::STORAGE_BUFFER)?;

    // ---- BLAS: uma por malha, montada uma vez ----
    let mut blas_addr = Vec::new();
    let mut builds = Vec::new(); // (geometria, primitivas, blas, endereço do rascunho)
    for &(io, ic, vo, vc) in &mb.meshes {
        let tri = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
            .vertex_format(vk::Format::R32G32B32_SFLOAT)
            .vertex_data(vk::DeviceOrHostAddressConstKHR { device_address: vb.addr + vo as u64 * 32 })
            .vertex_stride(32)
            .max_vertex(vc - 1)
            .index_type(vk::IndexType::UINT32)
            .index_data(vk::DeviceOrHostAddressConstKHR { device_address: ib.addr + io as u64 * 4 });
        let geo = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
            .geometry(vk::AccelerationStructureGeometryDataKHR { triangles: tri })
            .flags(vk::GeometryFlagsKHR::OPAQUE);
        let prims = ic / 3;
        let geos = [geo];
        let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(&geos);
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe {
            accel.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &info,
                &[prims],
                &mut sizes,
            )
        };
        let (blas, addr) = make_accel(&ctx, vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL, sizes.acceleration_structure_size)?;
        let (_scratch, scratch_addr) = scratch(&ctx, sizes.build_scratch_size)?;
        blas_addr.push(addr);
        builds.push((geo, prims, blas, scratch_addr));
    }
    ctx.one_shot(|cmd| {
        for (geo, prims, blas, scratch_addr) in &builds {
            let geos = [*geo];
            let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
                .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                .geometries(&geos)
                .dst_acceleration_structure(*blas)
                .scratch_data(vk::DeviceOrHostAddressKHR { device_address: *scratch_addr });
            let range = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(*prims)];
            unsafe { accel.cmd_build_acceleration_structures(cmd, &[info], &[&range]) };
        }
    })?;

    // ---- TLAS: refeita a cada frame (objetos girando) ----
    let inst_size = std::mem::size_of::<vk::AccelerationStructureInstanceKHR>() as u64;
    let inst_bufs = (0..IN_FLIGHT)
        .map(|_| {
            ctx.buffer(
                OBJECT_COUNT as u64 * inst_size,
                B::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR | B::SHADER_DEVICE_ADDRESS,
                true,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let tlas_geo = |addr: u64| {
        vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::INSTANCES)
            .geometry(vk::AccelerationStructureGeometryDataKHR {
                instances: vk::AccelerationStructureGeometryInstancesDataKHR::default()
                    .array_of_pointers(false)
                    .data(vk::DeviceOrHostAddressConstKHR { device_address: addr }),
            })
    };
    let tlas_flags = vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD;
    let mut tlas_sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    {
        let geos = [tlas_geo(inst_bufs[0].addr)];
        let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
            .flags(tlas_flags)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(&geos);
        unsafe {
            accel.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &info,
                &[OBJECT_COUNT as u32],
                &mut tlas_sizes,
            )
        };
    }
    let (tlas, _) = make_accel(&ctx, vk::AccelerationStructureTypeKHR::TOP_LEVEL, tlas_sizes.acceleration_structure_size)?;
    let (_tlas_scratch, tlas_scratch_addr) = scratch(&ctx, tlas_sizes.build_scratch_size)?;

    // ---- Saída (imagem de armazenamento) e descritores ----
    let output = ctx.image(
        w,
        h,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::SampleCountFlags::TYPE_1,
        vk::ImageAspectFlags::COLOR,
    )?;
    ctx.one_shot(|cmd| {
        crate::ctx::image_barrier(
            d,
            cmd,
            output.image,
            vk::ImageAspectFlags::COLOR,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::GENERAL,
            (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
            (vk::PipelineStageFlags2::COMPUTE_SHADER, vk::AccessFlags2::SHADER_STORAGE_WRITE),
        )
    })?;
    let ubos = (0..IN_FLIGHT)
        .map(|_| ctx.buffer(std::mem::size_of::<FrameUbo>() as u64, B::UNIFORM_BUFFER, true))
        .collect::<Result<Vec<_>>>()?;
    use vk::DescriptorType as T;
    let cs = vk::ShaderStageFlags::COMPUTE;
    let bindings = [
        (0, T::UNIFORM_BUFFER, cs),
        (1, T::ACCELERATION_STRUCTURE_KHR, cs),
        (2, T::STORAGE_BUFFER, cs),
        (3, T::STORAGE_BUFFER, cs),
        (4, T::STORAGE_BUFFER, cs),
        (5, T::STORAGE_BUFFER, cs),
        (6, T::STORAGE_IMAGE, cs),
    ];
    let dsl = ctx.set_layout(&bindings)?;
    let sets = ctx.alloc_sets(dsl, &bindings, IN_FLIGHT as u32)?;
    for (set, ubo) in sets.iter().zip(&ubos) {
        ctx.write_set(
            *set,
            &[
                (0, Bind::Ubo(ubo)),
                (1, Bind::Accel(tlas)),
                (2, Bind::Ssbo(&vb)),
                (3, Bind::Ssbo(&ib)),
                (4, Bind::Ssbo(&meshes_buf)),
                (5, Bind::Ssbo(&mats_buf)),
                (6, Bind::Storage(output.view)),
            ],
        );
    }
    let layout = ctx.pipeline_layout(&[dsl])?;
    let pipeline = ctx.compute(layout, spv!("rt.comp"))?;

    // ---- Fases: aquecimento + 3 passadas ----
    let total = o.seconds;
    // Aquecimento: 1/4 do total (de 2 s a 20 s); o resto em 3 passadas iguais. 30 s → 7,5 + 3 × 7,5.
    let warmup = (total / 4.0).min(20.0).max((total * 0.4).min(2.0));
    let pass = (total - warmup) / 3.0;
    let phases: Vec<(String, f64)> = std::iter::once(("aquecendo".to_string(), warmup))
        .chain((1..=3).map(|i| (format!("passada {i} de 3"), pass)))
        .collect();

    // Y para cima, como no D3D: o shader monta o raio com 1 - 2v.
    let proj = proj::directx::perspective(PI / 3.0, w as f32 / h as f32, 0.1, 2000.0);
    let sun = Vec3::new(-0.4, -0.75, -0.35).normalize();
    let mut insts = vec![vk::AccelerationStructureInstanceKHR {
        transform: vk::TransformMatrixKHR { matrix: [0.0; 12] },
        instance_custom_index_and_mask: vk::Packed24_8::new(0, 0xFF),
        instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(0, 0),
        acceleration_structure_reference: vk::AccelerationStructureReferenceKHR { device_handle: 0 },
    }; OBJECT_COUNT];
    let inst_flags = (vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE | vk::GeometryInstanceFlagsKHR::FORCE_OPAQUE)
        .as_raw() as u8;
    let src = Src {
        image: output.image,
        layout: vk::ImageLayout::GENERAL,
        stage: vk::PipelineStageFlags2::COMPUTE_SHADER,
        access: vk::AccessFlags2::SHADER_STORAGE_WRITE,
        w,
        h,
    };

    let mut looper = Looper::new(&ctx, win.size(), 30.0)?;
    let mut meter = Meter::default();
    let mut passes: Vec<Summary> = Vec::new();
    let mut partial = Summary::default();
    let mut frame_index = 0u32;
    let clock = Instant::now();
    let mut stopped = "tempo";

    'phases: for (pi, (phase, dur)) in phases.iter().enumerate() {
        let warm = pi == 0;
        let t0 = Instant::now();
        meter.times.clear();
        looper.reset_timing();
        loop {
            win.pump();
            if win.stop_requested() {
                stopped = "usuario";
                if !warm {
                    partial = meter.summary();
                }
                break 'phases;
            }
            let secs = t0.elapsed().as_secs_f64();
            if secs >= *dur {
                break;
            }
            let resized = win.take_resized();
            let f = looper.begin(resized, win.size())?;
            if let Some(ms) = f.gpu_ms {
                meter.push(ms);
            }

            // Câmera e objetos pelo tempo da passada: a mesma cena no mesmo instante em qualquer PC.
            let t = (secs / dur) as f32;
            let (eye, target) = camera_at(t);
            let vp = proj * look_at_mat4(eye, target, Vec3::Y);
            ubos[f.slot].write(&[FrameUbo {
                inv_view_proj: vp.inverse(),
                cam_time: eye.extend(secs as f32),
                sun_dir: sun.extend(0.0),
                p: [frame_index, w, h, spp],
                p2: [bounces, 0, 0, 0],
            }]);
            frame_index = frame_index.wrapping_add(1);
            for (i, (o, inst)) in objects.iter().zip(insts.iter_mut()).enumerate() {
                let m = obj_matrix(o, secs as f32);
                let (c0, c1, c2, c3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
                inst.transform.matrix = [c0.x, c1.x, c2.x, c3.x, c0.y, c1.y, c2.y, c3.y, c0.z, c1.z, c2.z, c3.z];
                inst.instance_custom_index_and_mask = vk::Packed24_8::new(i as u32, 0xFF);
                inst.instance_shader_binding_table_record_offset_and_flags = vk::Packed24_8::new(0, inst_flags);
                inst.acceleration_structure_reference.device_handle = blas_addr[o.mesh as usize];
            }
            let ib_slot = &inst_bufs[f.slot];
            unsafe {
                std::ptr::copy_nonoverlapping(insts.as_ptr() as *const u8, ib_slot.ptr, insts.len() * inst_size as usize)
            };

            let cmd = f.cmd;
            unsafe {
                let geos = [tlas_geo(ib_slot.addr)];
                let info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                    .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
                    .flags(tlas_flags)
                    .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                    .geometries(&geos)
                    .dst_acceleration_structure(tlas)
                    .scratch_data(vk::DeviceOrHostAddressKHR { device_address: tlas_scratch_addr });
                let range = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(OBJECT_COUNT as u32)];
                accel.cmd_build_acceleration_structures(cmd, &[info], &[&range]);
                crate::ctx::memory_barrier(
                    d,
                    cmd,
                    vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR,
                    vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR,
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR,
                );
                d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
                d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::COMPUTE, layout, 0, &[sets[f.slot]], &[]);
                d.cmd_dispatch(cmd, w.div_ceil(8), h.div_ceil(8), 1);
            }
            looper.end(f, &src)?;

            let now = clock.elapsed().as_secs_f64();
            if meter.tick(now) {
                out::progress(now, meter.fps, sensors.read(), json!({ "phase": phase }));
                win.set_title(&format!(
                    "RESET BENCH · Ray tracing · {w}x{h} · {preset} · {phase} · {:.0} FPS",
                    meter.fps
                ));
            }
        }
        if !warm {
            passes.push(meter.summary());
        }
    }
    looper.idle();
    let seconds = clock.elapsed().as_secs_f64();

    // Nota pela mediana das passadas completas (resiste a uma passada ruim). Parado antes da 1ª, vale a parcial.
    let measured: Vec<Summary> = if passes.is_empty() && partial.frames > 0 { vec![partial] } else { passes.clone() };
    let med_fps = median(measured.iter().map(|p| p.avg_fps).collect());
    let med_low = median(measured.iter().map(|p| p.low1_fps).collect());
    let frames: usize = measured.iter().map(|p| p.frames).sum();
    let variation = if measured.len() >= 2 && med_fps > 0.0 {
        let max = measured.iter().map(|p| p.avg_fps).fold(f64::MIN, f64::max);
        let min = measured.iter().map(|p| p.avg_fps).fold(f64::MAX, f64::min);
        (max - min) / med_fps * 100.0
    } else {
        0.0
    };
    let samples_per_sec = med_fps * w as f64 * h as f64 * spp as f64;
    // Raios por amostra: 1 de visibilidade/rebatida + 1 sombra + 1 oclusão por rebatida (estimativa, como no Windows).
    let gigarays = samples_per_sec * bounces as f64 * 3.0 / 1e9;
    let s = sensors.read();
    out::result(
        "rt",
        Summary { avg_fps: med_fps, low1_fps: med_low, frames },
        seconds,
        Some(samples_per_sec / 10_000.0),
        stopped,
        json!({
            "preset": preset,
            "width": w,
            "height": h,
            "spp": spp,
            "bounces": bounces,
            "passes": measured.iter().map(|p| json!({
                "avg_fps": out::round(p.avg_fps, 1),
                "low1_fps": out::round(p.low1_fps, 1),
                "frames": p.frames,
            })).collect::<Vec<_>>(),
            "variation_pct": out::round(variation, 1),
            "reliable": passes.len() >= 3 && variation < 3.0,
            "samples_per_sec": samples_per_sec.round() as u64,
            "gigarays": out::round(gigarays, 2),
            "max_temp_c": s.max_temp_c.map(|t| t.round() as i64),
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mediana_e_cena() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(build_objects().len(), OBJECT_COUNT);
        let mut mb = MeshBuilder::default();
        mb.sphere(160);
        assert_eq!(mb.i.len() / 3, 80 * 160 * 2);
    }
}
