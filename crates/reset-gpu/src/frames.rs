//! Laço de frames comum aos testes, com a mesma técnica do app do Windows para não ficar preso ao V-Sync:
//! render sempre numa imagem própria (fora da tela), vários frames em voo, e a janela só recebe uma cópia
//! a ~60 Hz (ou ~30 Hz no ray tracing). A apresentação nunca espera: se a tela não liberou imagem, pula.
//! O tempo de cada frame vem dos timestamps da própria GPU (intervalo entre o fim de dois frames seguidos).

use crate::ctx::{image_barrier, memory_barrier, Ctx};
use anyhow::{bail, Context, Result};
use ash::{khr, vk};
use std::time::Instant;

/// Frames em voo: a CPU fica até 4 frames à frente da GPU, para ela nunca ficar ociosa.
pub const IN_FLIGHT: usize = 4;

/// A imagem que vai para a janela e como ela foi escrita por último.
pub struct Src {
    pub image: vk::Image,
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
    pub w: u32,
    pub h: u32,
}

pub struct Frame {
    pub slot: usize,
    pub cmd: vk::CommandBuffer,
    /// Tempo de GPU do frame que ocupava este slot (ms), quando dá para medir.
    pub gpu_ms: Option<f64>,
}

/// Swapchain da janela. Modo sem V-Sync (MAILBOX ou IMMEDIATE) quando o driver oferece.
struct Presenter {
    device: ash::Device,
    surface_fn: khr::surface::Instance,
    sc_fn: khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    pdev: vk::PhysicalDevice,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    extent: vk::Extent2D,
    /// Um por slot de frame (sinalizado quando a imagem da janela está livre).
    acquire: Vec<vk::Semaphore>,
    /// Um por imagem da janela (sinalizado quando a cópia terminou).
    done: Vec<vk::Semaphore>,
    dirty: bool,
    interval: f64,
    last: Option<Instant>,
}

impl Presenter {
    fn new(ctx: &Ctx, size: (u32, u32), interval: f64) -> Result<Presenter> {
        let mut acquire = Vec::new();
        for _ in 0..IN_FLIGHT {
            acquire.push(unsafe { ctx.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }?);
        }
        let mut p = Presenter {
            device: ctx.device.clone(),
            surface_fn: ctx.surface_fn.clone(),
            sc_fn: khr::swapchain::Device::new(&ctx.instance, &ctx.device),
            surface: ctx.surface,
            pdev: ctx.pdev,
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            extent: vk::Extent2D::default(),
            acquire,
            done: Vec::new(),
            dirty: false,
            interval,
            last: None,
        };
        p.recreate(size)?;
        Ok(p)
    }

    /// (Re)cria a swapchain no tamanho da janela. Chamar com a GPU parada.
    fn recreate(&mut self, size: (u32, u32)) -> Result<()> {
        self.dirty = false;
        for s in self.done.drain(..) {
            unsafe { self.device.destroy_semaphore(s, None) };
        }
        let old = std::mem::replace(&mut self.swapchain, vk::SwapchainKHR::null());
        self.images.clear();
        let caps = unsafe { self.surface_fn.get_physical_device_surface_capabilities(self.pdev, self.surface) }?;
        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: size.0.clamp(caps.min_image_extent.width, caps.max_image_extent.width.max(1)),
                height: size.1.clamp(caps.min_image_extent.height, caps.max_image_extent.height.max(1)),
            }
        };
        if extent.width == 0 || extent.height == 0 {
            // Janela minimizada: o teste continua, só não mostra nada.
            if old != vk::SwapchainKHR::null() {
                unsafe { self.sc_fn.destroy_swapchain(old, None) };
            }
            return Ok(());
        }
        if !caps.supported_usage_flags.contains(vk::ImageUsageFlags::TRANSFER_DST) {
            bail!("a janela não aceita cópia de imagem (swapchain sem TRANSFER_DST)");
        }
        let formats = unsafe { self.surface_fn.get_physical_device_surface_formats(self.pdev, self.surface) }?;
        // Formato linear (UNORM): os shaders já aplicam a gama.
        let format = formats
            .iter()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM || f.format == vk::Format::R8G8B8A8_UNORM)
            .or_else(|| formats.first())
            .copied()
            .context("a janela não oferece nenhum formato de imagem")?;
        let modes = unsafe { self.surface_fn.get_physical_device_surface_present_modes(self.pdev, self.surface) }?;
        let mode = [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
            .into_iter()
            .find(|m| modes.contains(m))
            .unwrap_or(vk::PresentModeKHR::FIFO);
        let mut count = (caps.min_image_count + 1).max(3);
        if caps.max_image_count > 0 {
            count = count.min(caps.max_image_count);
        }
        let alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::INHERIT,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        ]
        .into_iter()
        .find(|a| caps.supported_composite_alpha.contains(*a))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(alpha)
            .present_mode(mode)
            .clipped(true)
            .old_swapchain(old);
        let sc = unsafe { self.sc_fn.create_swapchain(&info, None) };
        if old != vk::SwapchainKHR::null() {
            unsafe { self.sc_fn.destroy_swapchain(old, None) };
        }
        self.swapchain = sc.context("não foi possível criar a swapchain da janela")?;
        self.images = unsafe { self.sc_fn.get_swapchain_images(self.swapchain) }?;
        for _ in 0..self.images.len() {
            self.done.push(unsafe { self.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }?);
        }
        self.extent = extent;
        Ok(())
    }

    fn due(&self) -> bool {
        self.swapchain != vk::SwapchainKHR::null()
            && !self.dirty
            && self.last.is_none_or(|l| l.elapsed().as_secs_f64() >= self.interval)
    }

    /// Pega uma imagem da janela sem esperar. None se a tela ainda não liberou nenhuma.
    fn acquire(&mut self, slot: usize) -> Result<Option<u32>> {
        let r = unsafe {
            self.sc_fn.acquire_next_image(self.swapchain, 0, self.acquire[slot], vk::Fence::null())
        };
        match r {
            Ok((idx, suboptimal)) => {
                self.dirty |= suboptimal;
                self.last = Some(Instant::now());
                Ok(Some(idx))
            }
            Err(vk::Result::NOT_READY) | Err(vk::Result::TIMEOUT) => Ok(None),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.dirty = true;
                Ok(None)
            }
            Err(e) => Err(e).context("falha ao pegar imagem da janela"),
        }
    }

    /// Copia a imagem renderizada para a da janela, ajustando a escala e mantendo a proporção (tarjas pretas).
    fn blit(&self, cmd: vk::CommandBuffer, src: &Src, idx: u32) {
        let d = &self.device;
        let dst = self.images[idx as usize];
        let color = vk::ImageAspectFlags::COLOR;
        let xfer = vk::PipelineStageFlags2::ALL_TRANSFER;
        let src_layout = if src.layout == vk::ImageLayout::GENERAL {
            vk::ImageLayout::GENERAL
        } else {
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL
        };
        image_barrier(d, cmd, src.image, color, src.layout, src_layout, (src.stage, src.access), (xfer, vk::AccessFlags2::TRANSFER_READ));
        image_barrier(
            d,
            cmd,
            dst,
            color,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            (xfer, vk::AccessFlags2::NONE),
            (xfer, vk::AccessFlags2::TRANSFER_WRITE),
        );
        let (ew, eh) = (self.extent.width as f64, self.extent.height as f64);
        let scale = (ew / src.w as f64).min(eh / src.h as f64);
        let (w, h) = ((src.w as f64 * scale).round() as i32, (src.h as f64 * scale).round() as i32);
        let (x, y) = ((ew as i32 - w) / 2, (eh as i32 - h) / 2);
        unsafe {
            if x > 0 || y > 0 {
                d.cmd_clear_color_image(
                    cmd,
                    dst,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] },
                    &[crate::ctx::range(color)],
                );
                memory_barrier(d, cmd, xfer, vk::AccessFlags2::TRANSFER_WRITE, xfer, vk::AccessFlags2::TRANSFER_WRITE);
            }
            let layers = vk::ImageSubresourceLayers { aspect_mask: color, mip_level: 0, base_array_layer: 0, layer_count: 1 };
            let region = vk::ImageBlit {
                src_subresource: layers,
                src_offsets: [vk::Offset3D { x: 0, y: 0, z: 0 }, vk::Offset3D { x: src.w as i32, y: src.h as i32, z: 1 }],
                dst_subresource: layers,
                dst_offsets: [vk::Offset3D { x, y, z: 0 }, vk::Offset3D { x: x + w, y: y + h, z: 1 }],
            };
            d.cmd_blit_image(cmd, src.image, src_layout, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region], vk::Filter::LINEAR);
        }
        image_barrier(
            d,
            cmd,
            dst,
            color,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::PRESENT_SRC_KHR,
            (xfer, vk::AccessFlags2::TRANSFER_WRITE),
            (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
        );
    }

    fn present(&mut self, queue: vk::Queue, idx: u32) -> Result<()> {
        let wait = [self.done[idx as usize]];
        let scs = [self.swapchain];
        let idxs = [idx];
        let info = vk::PresentInfoKHR::default().wait_semaphores(&wait).swapchains(&scs).image_indices(&idxs);
        match unsafe { self.sc_fn.queue_present(queue, &info) } {
            Ok(suboptimal) => self.dirty |= suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.dirty = true,
            Err(e) => return Err(e).context("falha ao mostrar o frame na janela"),
        }
        Ok(())
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            for s in self.done.drain(..).chain(self.acquire.drain(..)) {
                self.device.destroy_semaphore(s, None);
            }
            if self.swapchain != vk::SwapchainKHR::null() {
                self.sc_fn.destroy_swapchain(self.swapchain, None);
            }
        }
    }
}

/// Buffers de comando, cercas e timestamps dos frames em voo, mais a apresentação na janela.
pub struct Looper {
    device: ash::Device,
    queue: vk::Queue,
    cmds: Vec<vk::CommandBuffer>,
    fences: Vec<vk::Fence>,
    queries: vk::QueryPool,
    /// Milissegundos por tique do timestamp; None se a fila não tem timestamps (aí mede pela CPU).
    tick_ms: Option<f64>,
    ts_mask: u64,
    last_ts: Option<u64>,
    last_cpu: Option<Instant>,
    frame: u64,
    pres: Presenter,
}

impl Looper {
    /// `present_hz`: quantas vezes por segundo a janela recebe uma cópia do frame.
    pub fn new(ctx: &Ctx, size: (u32, u32), present_hz: f64) -> Result<Looper> {
        let d = &ctx.device;
        let cmds = unsafe {
            d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(ctx.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(IN_FLIGHT as u32),
            )
        }?;
        let mut fences = Vec::new();
        for _ in 0..IN_FLIGHT {
            fences.push(unsafe { d.create_fence(&vk::FenceCreateInfo::default(), None) }?);
        }
        let queries = unsafe {
            d.create_query_pool(
                &vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(IN_FLIGHT as u32),
                None,
            )
        }?;
        let bits = ctx.timestamp_bits;
        Ok(Looper {
            device: d.clone(),
            queue: ctx.queue,
            cmds,
            fences,
            queries,
            tick_ms: (bits > 0).then(|| ctx.limits.timestamp_period as f64 / 1e6),
            ts_mask: if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 },
            last_ts: None,
            last_cpu: None,
            frame: 0,
            pres: Presenter::new(ctx, size, 1.0 / present_hz)?,
        })
    }

    /// Descarta o intervalo em curso (troca de fase ou de swapchain não vira um frame lento).
    pub fn reset_timing(&mut self) {
        self.last_ts = None;
        self.last_cpu = None;
    }

    /// Começa um frame: recria a swapchain se a janela mudou, espera o slot livre e lê o tempo do frame anterior nele.
    pub fn begin(&mut self, resized: bool, size: (u32, u32)) -> Result<Frame> {
        if resized {
            self.pres.dirty = true;
        }
        if self.pres.dirty {
            unsafe { self.device.device_wait_idle() }?;
            self.pres.recreate(size)?;
            self.reset_timing();
        }
        let d = &self.device;
        let slot = (self.frame % IN_FLIGHT as u64) as usize;
        let mut gpu_ms = None;
        if self.frame >= IN_FLIGHT as u64 {
            unsafe { d.wait_for_fences(&[self.fences[slot]], true, u64::MAX) }
                .context("a placa de vídeo parou de responder")?;
            if let Some(tick) = self.tick_ms {
                let mut ts = [0u64; 1];
                unsafe { d.get_query_pool_results(self.queries, slot as u32, &mut ts, vk::QueryResultFlags::TYPE_64) }?;
                let t = ts[0] & self.ts_mask;
                if let Some(prev) = self.last_ts.filter(|&p| t > p) {
                    gpu_ms = Some((t - prev) as f64 * tick);
                }
                self.last_ts = Some(t);
            } else {
                let now = Instant::now();
                gpu_ms = self.last_cpu.map(|p| (now - p).as_secs_f64() * 1000.0);
                self.last_cpu = Some(now);
            }
        }
        let cmd = self.cmds[slot];
        unsafe {
            d.reset_fences(&[self.fences[slot]])?;
            d.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
            d.cmd_reset_query_pool(cmd, self.queries, slot as u32, 1);
        }
        // Os recursos são compartilhados entre os frames em voo: o frame anterior termina antes deste começar.
        memory_barrier(
            d,
            cmd,
            vk::PipelineStageFlags2::ALL_COMMANDS,
            vk::AccessFlags2::MEMORY_WRITE,
            vk::PipelineStageFlags2::ALL_COMMANDS,
            vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
        );
        Ok(Frame { slot, cmd, gpu_ms })
    }

    /// Fecha o frame: copia para a janela (se for a hora e houver imagem livre), marca o timestamp e envia.
    pub fn end(&mut self, f: Frame, src: &Src) -> Result<()> {
        let d = &self.device;
        let mut idx = None;
        if self.pres.due() {
            idx = self.pres.acquire(f.slot)?;
            if let Some(i) = idx {
                self.pres.blit(f.cmd, src, i);
            }
        }
        unsafe {
            d.cmd_write_timestamp2(f.cmd, vk::PipelineStageFlags2::ALL_COMMANDS, self.queries, f.slot as u32);
            d.end_command_buffer(f.cmd)?;
        }
        let cmds = [vk::CommandBufferSubmitInfo::default().command_buffer(f.cmd)];
        let waits: Vec<_> = idx
            .map(|_| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(self.pres.acquire[f.slot])
                    .stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
            })
            .into_iter()
            .collect();
        let signals: Vec<_> = idx
            .map(|i| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(self.pres.done[i as usize])
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .into_iter()
            .collect();
        let submit = vk::SubmitInfo2::default()
            .command_buffer_infos(&cmds)
            .wait_semaphore_infos(&waits)
            .signal_semaphore_infos(&signals);
        unsafe { d.queue_submit2(self.queue, &[submit], self.fences[f.slot]) }
            .context("a placa de vídeo recusou o frame (vkQueueSubmit2)")?;
        if let Some(i) = idx {
            self.pres.present(self.queue, i)?;
        }
        self.frame += 1;
        Ok(())
    }

    /// Se este frame deve ir para a janela (para preparar a imagem só quando precisa).
    pub fn present_due(&self) -> bool {
        self.pres.due()
    }

    /// Espera a GPU terminar tudo o que foi enviado.
    pub fn idle(&self) {
        unsafe {
            let _ = self.device.device_wait_idle();
        }
    }
}

impl Drop for Looper {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            for f in self.fences.drain(..) {
                self.device.destroy_fence(f, None);
            }
            self.device.destroy_query_pool(self.queries, None);
        }
    }
}
