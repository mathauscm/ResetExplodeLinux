//! Base Vulkan dos três testes: instância, escolha da placa, dispositivo com uma fila só, memória e utilitários
//! (buffers, imagens, pipelines, descritores, barreiras). Tudo que é criado aqui entra numa lista e é destruído junto
//! com o contexto, na ordem inversa (como a lista `keep` do app do Windows).

use anyhow::{anyhow, bail, Context, Result};
use ash::{khr, vk};
use bytemuck::Pod;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::cell::RefCell;
use std::ffi::c_char;
use std::io::Cursor;

/// O que o `info` mostra da placa escolhida.
pub struct Adapter {
    pub name: String,
    pub driver: String,
    /// Versão do Vulkan que o driver oferece, ex.: "1.4.312".
    pub api: String,
    /// Maior heap de memória local da placa, em bytes.
    pub vram: u64,
    /// Ray tracing por hardware com raios inline (VK_KHR_ray_query + VK_KHR_acceleration_structure).
    pub ray_query: bool,
}

/// Recursos criados pelo contexto, destruídos na ordem inversa no Drop.
enum Res {
    Buffer(vk::Buffer),
    Image(vk::Image),
    View(vk::ImageView),
    Memory(vk::DeviceMemory),
    Sampler(vk::Sampler),
    Shader(vk::ShaderModule),
    Pipeline(vk::Pipeline),
    Layout(vk::PipelineLayout),
    SetLayout(vk::DescriptorSetLayout),
    Pool(vk::DescriptorPool),
    Accel(vk::AccelerationStructureKHR),
}

pub struct Buffer {
    pub buf: vk::Buffer,
    pub size: u64,
    /// Ponteiro mapeado (só nos buffers visíveis pela CPU; nulo nos outros).
    pub ptr: *mut u8,
    /// Endereço na GPU (só com SHADER_DEVICE_ADDRESS; 0 nos outros).
    pub addr: u64,
}

impl Buffer {
    /// Copia dados para um buffer mapeado.
    pub fn write<T: Pod>(&self, data: &[T]) {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        assert!(!self.ptr.is_null() && bytes.len() as u64 <= self.size, "buffer não mapeado ou pequeno demais");
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr, bytes.len()) };
    }
}

pub struct Image {
    pub image: vk::Image,
    pub view: vk::ImageView,
}

/// Descrição de um pipeline gráfico (viewport e scissor são dinâmicos; renderização dinâmica, sem render pass).
#[derive(Default)]
pub struct Gfx<'a> {
    pub vs: &'a [u8],
    pub fs: Option<&'a [u8]>,
    /// Bytes por vértice; 0 = sem vertex buffer (triângulo de tela cheia).
    pub stride: u32,
    /// (location, formato, offset) de cada atributo.
    pub attrs: &'a [(u32, vk::Format, u32)],
    pub color: Option<vk::Format>,
    pub depth: Option<vk::Format>,
    pub depth_test: bool,
    pub depth_write: bool,
    /// (constante, inclinação) do depth bias, para o mapa de sombras.
    pub bias: Option<(f32, f32)>,
    pub blend: bool,
    pub samples: Option<vk::SampleCountFlags>,
}

/// Ligação de um descritor, para `write_set`.
pub enum Bind<'a> {
    Ubo(&'a Buffer),
    Ssbo(&'a Buffer),
    Tex(vk::ImageView, vk::Sampler),
    Storage(vk::ImageView),
    Accel(vk::AccelerationStructureKHR),
}

pub struct Ctx {
    _entry: ash::Entry,
    pub instance: ash::Instance,
    pub surface_fn: khr::surface::Instance,
    pub surface: vk::SurfaceKHR,
    pub pdev: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub adapter: Adapter,
    pub limits: vk::PhysicalDeviceLimits,
    pub timestamp_bits: u32,
    mem_props: vk::PhysicalDeviceMemoryProperties,
    /// Funções de estrutura de aceleração (só quando o contexto foi criado para ray tracing).
    pub accel: Option<khr::acceleration_structure::Device>,
    pub scratch_align: u64,
    pub pool: vk::CommandPool,
    res: RefCell<Vec<Res>>,
}

fn load_entry() -> Result<ash::Entry> {
    unsafe { ash::Entry::load() }
        .context("Vulkan não encontrado (libvulkan.so.1). Instale vulkan-icd-loader e o driver Vulkan da placa")
}

fn create_instance(entry: &ash::Entry, exts: &[*const c_char]) -> Result<ash::Instance> {
    let app = vk::ApplicationInfo::default()
        .application_name(c"reset-gpu")
        .engine_name(c"RESET EXPLODE")
        .api_version(vk::API_VERSION_1_3);
    let info = vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(exts);
    unsafe { entry.create_instance(&info, None) }.context("não foi possível iniciar o Vulkan (vkCreateInstance)")
}

fn has_ext(list: &[vk::ExtensionProperties], name: &std::ffi::CStr) -> bool {
    list.iter().any(|e| e.extension_name_as_c_str() == Ok(name))
}

/// Ray query por hardware: as extensões existem e os recursos estão ligados no driver.
fn ray_query_supported(instance: &ash::Instance, pd: vk::PhysicalDevice) -> bool {
    let Ok(exts) = (unsafe { instance.enumerate_device_extension_properties(pd) }) else { return false };
    if !has_ext(&exts, khr::ray_query::NAME)
        || !has_ext(&exts, khr::acceleration_structure::NAME)
        || !has_ext(&exts, khr::deferred_host_operations::NAME)
    {
        return false;
    }
    let mut asf = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
    let mut rq = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
    let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut asf).push_next(&mut rq).push_next(&mut v12);
    unsafe { instance.get_physical_device_features2(pd, &mut f2) };
    asf.acceleration_structure == vk::TRUE && rq.ray_query == vk::TRUE && v12.buffer_device_address == vk::TRUE
}

/// Escolhe a placa: dedicada antes de integrada, depois a com mais memória de vídeo (como o app do Windows).
/// Precisa de Vulkan 1.3 e de uma fila de gráficos + compute (que apresente na janela, se houver janela).
fn pick(
    instance: &ash::Instance,
    surface: Option<(&khr::surface::Instance, vk::SurfaceKHR)>,
) -> Result<(vk::PhysicalDevice, u32, Adapter)> {
    let pdevs = unsafe { instance.enumerate_physical_devices() }.context("nenhuma placa de vídeo com Vulkan")?;
    let mut best: Option<(u64, vk::PhysicalDevice, u32, Adapter)> = None;
    let mut old_api = None;
    for pd in pdevs {
        let props = unsafe { instance.get_physical_device_properties(pd) };
        let name = props.device_name_as_c_str().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if props.device_type == vk::PhysicalDeviceType::CPU {
            continue; // llvmpipe e afins: renderização por software não serve para medir GPU
        }
        if props.api_version < vk::API_VERSION_1_3 {
            old_api = Some(name);
            continue;
        }
        let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
        let family = families.iter().enumerate().position(|(i, q)| {
            q.queue_flags.contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                && surface.is_none_or(|(sf, s)| unsafe {
                    sf.get_physical_device_surface_support(pd, i as u32, s).unwrap_or(false)
                })
        });
        let Some(family) = family else { continue };

        let mem = unsafe { instance.get_physical_device_memory_properties(pd) };
        let vram = mem.memory_heaps[..mem.memory_heap_count as usize]
            .iter()
            .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
            .map(|h| h.size)
            .max()
            .unwrap_or(0);
        let mut drv = vk::PhysicalDeviceDriverProperties::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut drv);
        unsafe { instance.get_physical_device_properties2(pd, &mut p2) };
        let driver = drv
            .driver_info_as_c_str()
            .ok()
            .map(|s| s.to_string_lossy().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| decode_driver(props.vendor_id, props.driver_version));
        let api = format!(
            "{}.{}.{}",
            vk::api_version_major(props.api_version),
            vk::api_version_minor(props.api_version),
            vk::api_version_patch(props.api_version)
        );
        let discrete = props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
        let score = if discrete { 1 << 62 } else { 0 } + vram;
        let adapter = Adapter { name, driver, api, vram, ray_query: ray_query_supported(instance, pd) };
        if best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, pd, family as u32, adapter));
        }
    }
    match best {
        Some((_, pd, family, adapter)) => Ok((pd, family, adapter)),
        None => match old_api {
            Some(n) => bail!("{n}: o driver só oferece Vulkan anterior ao 1.3. Atualize o driver da placa"),
            None => bail!("nenhuma placa de vídeo com Vulkan que possa desenhar nesta janela"),
        },
    }
}

/// Versão do driver quando ele não informa o texto pronto (codificação própria da NVIDIA).
fn decode_driver(vendor: u32, v: u32) -> String {
    if vendor == 0x10DE {
        format!("{}.{:02}", (v >> 22) & 0x3FF, (v >> 14) & 0xFF)
    } else {
        format!("{}.{}.{}", vk::api_version_major(v), vk::api_version_minor(v), vk::api_version_patch(v))
    }
}

/// Só a ficha da placa, sem janela (para o `reset-gpu info`).
pub fn probe() -> Result<Adapter> {
    let entry = load_entry()?;
    let instance = create_instance(&entry, &[])?;
    let res = pick(&instance, None);
    unsafe { instance.destroy_instance(None) };
    Ok(res?.2)
}

impl Ctx {
    /// Contexto ligado à janela. Com `want_rt`, liga ray query e estruturas de aceleração (ou falha com o motivo).
    pub fn new(window: &winit::window::Window, want_rt: bool) -> Result<Ctx> {
        let entry = load_entry()?;
        let dh = window.display_handle().map_err(|e| anyhow!("janela sem display: {e}"))?.as_raw();
        let wh = window.window_handle().map_err(|e| anyhow!("janela sem handle: {e}"))?.as_raw();
        let exts = ash_window::enumerate_required_extensions(dh).context("Vulkan sem suporte a janelas neste sistema")?;
        let instance = create_instance(&entry, exts)?;
        let surface = unsafe { ash_window::create_surface(&entry, &instance, dh, wh, None) }
            .context("não foi possível ligar o Vulkan à janela (vkCreate*SurfaceKHR)")?;
        let surface_fn = khr::surface::Instance::new(&entry, &instance);
        let (pdev, family, adapter) = pick(&instance, Some((&surface_fn, surface)))?;
        if want_rt && !adapter.ray_query {
            bail!(
                "{}: sem ray tracing por hardware no Vulkan (VK_KHR_ray_query). Precisa de RTX 20 ou mais nova, \
                 Radeon RX 6000 ou mais nova, ou Intel Arc, com driver atualizado",
                adapter.name
            );
        }

        let mut dev_exts = vec![khr::swapchain::NAME.as_ptr()];
        if want_rt {
            dev_exts.extend([
                khr::acceleration_structure::NAME.as_ptr(),
                khr::ray_query::NAME.as_ptr(),
                khr::deferred_host_operations::NAME.as_ptr(),
            ]);
        }
        let prio = [1.0f32];
        let qinfo = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&prio)];
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default().buffer_device_address(want_rt);
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default().dynamic_rendering(true)
            .synchronization2(true)
            .shader_demote_to_helper_invocation(true); // o `discard` do GLSL vira demote no SPIR-V 1.6
        let mut fas = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default().acceleration_structure(true);
        let mut frq = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
        let mut info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&qinfo)
            .enabled_extension_names(&dev_exts)
            .push_next(&mut f12)
            .push_next(&mut f13);
        if want_rt {
            info = info.push_next(&mut fas).push_next(&mut frq);
        }
        let device = unsafe { instance.create_device(pdev, &info, None) }
            .context("não foi possível abrir a placa de vídeo (vkCreateDevice)")?;
        let queue = unsafe { device.get_device_queue(family, 0) };

        let props = unsafe { instance.get_physical_device_properties(pdev) };
        let mem_props = unsafe { instance.get_physical_device_memory_properties(pdev) };
        let families = unsafe { instance.get_physical_device_queue_family_properties(pdev) };
        let timestamp_bits = if props.limits.timestamp_compute_and_graphics == vk::TRUE {
            families[family as usize].timestamp_valid_bits
        } else {
            0
        };
        let (accel, scratch_align) = if want_rt {
            let mut asp = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
            let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut asp);
            unsafe { instance.get_physical_device_properties2(pdev, &mut p2) };
            let align = (asp.min_acceleration_structure_scratch_offset_alignment as u64).max(1);
            (Some(khr::acceleration_structure::Device::new(&instance, &device)), align)
        } else {
            (None, 1)
        };
        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }?;

        Ok(Ctx {
            _entry: entry,
            instance,
            surface_fn,
            surface,
            pdev,
            device,
            queue,
            adapter,
            limits: props.limits,
            timestamp_bits,
            mem_props,
            accel,
            scratch_align,
            pool,
            res: RefCell::new(Vec::new()),
        })
    }

    fn keep(&self, r: Res) {
        self.res.borrow_mut().push(r);
    }

    pub fn keep_accel(&self, a: vk::AccelerationStructureKHR) {
        self.keep(Res::Accel(a));
    }

    fn alloc(&self, req: vk::MemoryRequirements, flags: vk::MemoryPropertyFlags, address: bool) -> Result<vk::DeviceMemory> {
        let types = &self.mem_props.memory_types[..self.mem_props.memory_type_count as usize];
        let index = types
            .iter()
            .enumerate()
            .position(|(i, t)| req.memory_type_bits & (1 << i) != 0 && t.property_flags.contains(flags))
            .ok_or_else(|| anyhow!("nenhum tipo de memória com {flags:?}"))?;
        let mut fl = vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let mut info = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(index as u32);
        if address {
            info = info.push_next(&mut fl);
        }
        let mem = unsafe { self.device.allocate_memory(&info, None) }
            .with_context(|| format!("sem memória de vídeo para {} MB", req.size >> 20))?;
        self.keep(Res::Memory(mem));
        Ok(mem)
    }

    /// Buffer na memória da placa (`host` = false) ou visível e já mapeado pela CPU (`host` = true).
    pub fn buffer(&self, size: u64, usage: vk::BufferUsageFlags, host: bool) -> Result<Buffer> {
        let size = size.max(16);
        let buf = unsafe {
            self.device.create_buffer(
                &vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }?;
        self.keep(Res::Buffer(buf));
        let req = unsafe { self.device.get_buffer_memory_requirements(buf) };
        let flags = if host {
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
        } else {
            vk::MemoryPropertyFlags::DEVICE_LOCAL
        };
        let address = usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS);
        let mem = self.alloc(req, flags, address)?;
        unsafe { self.device.bind_buffer_memory(buf, mem, 0) }?;
        let ptr = if host {
            unsafe { self.device.map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) }? as *mut u8
        } else {
            std::ptr::null_mut()
        };
        let addr = if address {
            unsafe { self.device.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf)) }
        } else {
            0
        };
        Ok(Buffer { buf, size, ptr, addr })
    }

    /// Buffer na memória da placa já com os dados (copiados por um buffer intermediário).
    pub fn upload<T: Pod>(&self, data: &[T], usage: vk::BufferUsageFlags) -> Result<Buffer> {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let staging = self.buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        staging.write(bytes);
        let dst = self.buffer(bytes.len() as u64, usage | vk::BufferUsageFlags::TRANSFER_DST, false)?;
        self.one_shot(|cmd| unsafe {
            self.device.cmd_copy_buffer(
                cmd,
                staging.buf,
                dst.buf,
                &[vk::BufferCopy { src_offset: 0, dst_offset: 0, size: bytes.len() as u64 }],
            )
        })?;
        Ok(dst)
    }

    pub fn image(
        &self,
        w: u32,
        h: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        samples: vk::SampleCountFlags,
        aspect: vk::ImageAspectFlags,
    ) -> Result<Image> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: w, height: h, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(samples)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&info, None) }
            .with_context(|| format!("não foi possível criar a imagem {w}x{h} {format:?}"))?;
        self.keep(Res::Image(image));
        let req = unsafe { self.device.get_image_memory_requirements(image) };
        let mem = self.alloc(req, vk::MemoryPropertyFlags::DEVICE_LOCAL, false)?;
        unsafe { self.device.bind_image_memory(image, mem, 0) }?;
        // Imagem só de cópia (destino de resolve do MSAA) não tem view.
        let viewable = vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
        if !usage.intersects(viewable) {
            return Ok(Image { image, view: vk::ImageView::null() });
        }
        let view = unsafe {
            self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(range(aspect)),
                None,
            )
        }?;
        self.keep(Res::View(view));
        Ok(Image { image, view })
    }

    pub fn sampler(&self, info: &vk::SamplerCreateInfo) -> Result<vk::Sampler> {
        let s = unsafe { self.device.create_sampler(info, None) }?;
        self.keep(Res::Sampler(s));
        Ok(s)
    }

    fn shader(&self, spv: &[u8]) -> Result<vk::ShaderModule> {
        let code = ash::util::read_spv(&mut Cursor::new(spv)).context("SPIR-V inválido")?;
        let m = unsafe { self.device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None) }?;
        self.keep(Res::Shader(m));
        Ok(m)
    }

    pub fn set_layout(&self, bindings: &[(u32, vk::DescriptorType, vk::ShaderStageFlags)]) -> Result<vk::DescriptorSetLayout> {
        let b: Vec<_> = bindings
            .iter()
            .map(|&(binding, ty, stages)| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(ty)
                    .descriptor_count(1)
                    .stage_flags(stages)
            })
            .collect();
        let l = unsafe {
            self.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&b), None)
        }?;
        self.keep(Res::SetLayout(l));
        Ok(l)
    }

    /// `count` conjuntos de descritores com o mesmo layout (um por frame em voo), num pool próprio.
    pub fn alloc_sets(
        &self,
        layout: vk::DescriptorSetLayout,
        bindings: &[(u32, vk::DescriptorType, vk::ShaderStageFlags)],
        count: u32,
    ) -> Result<Vec<vk::DescriptorSet>> {
        let sizes: Vec<_> = bindings
            .iter()
            .map(|&(_, ty, _)| vk::DescriptorPoolSize { ty, descriptor_count: count })
            .collect();
        let pool = unsafe {
            self.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default().max_sets(count).pool_sizes(&sizes),
                None,
            )
        }?;
        self.keep(Res::Pool(pool));
        let layouts = vec![layout; count as usize];
        Ok(unsafe {
            self.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts),
            )
        }?)
    }

    pub fn write_set(&self, set: vk::DescriptorSet, binds: &[(u32, Bind)]) {
        for (binding, b) in binds {
            let w = vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*binding);
            unsafe {
                match b {
                    Bind::Ubo(buf) | Bind::Ssbo(buf) => {
                        let ty = if matches!(b, Bind::Ubo(_)) {
                            vk::DescriptorType::UNIFORM_BUFFER
                        } else {
                            vk::DescriptorType::STORAGE_BUFFER
                        };
                        let info = [vk::DescriptorBufferInfo { buffer: buf.buf, offset: 0, range: vk::WHOLE_SIZE }];
                        self.device.update_descriptor_sets(&[w.descriptor_type(ty).buffer_info(&info)], &[]);
                    }
                    Bind::Tex(view, sampler) => {
                        let info = [vk::DescriptorImageInfo {
                            sampler: *sampler,
                            image_view: *view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }];
                        let w = w.descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info);
                        self.device.update_descriptor_sets(&[w], &[]);
                    }
                    Bind::Storage(view) => {
                        let info = [vk::DescriptorImageInfo {
                            sampler: vk::Sampler::null(),
                            image_view: *view,
                            image_layout: vk::ImageLayout::GENERAL,
                        }];
                        let w = w.descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(&info);
                        self.device.update_descriptor_sets(&[w], &[]);
                    }
                    Bind::Accel(a) => {
                        let list = [*a];
                        let mut info = vk::WriteDescriptorSetAccelerationStructureKHR::default().acceleration_structures(&list);
                        let w = w
                            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                            .descriptor_count(1)
                            .push_next(&mut info);
                        self.device.update_descriptor_sets(&[w], &[]);
                    }
                }
            }
        }
    }

    pub fn pipeline_layout(&self, sets: &[vk::DescriptorSetLayout]) -> Result<vk::PipelineLayout> {
        let l = unsafe {
            self.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(sets), None)
        }?;
        self.keep(Res::Layout(l));
        Ok(l)
    }

    pub fn graphics(&self, layout: vk::PipelineLayout, d: &Gfx) -> Result<vk::Pipeline> {
        let vs = self.shader(d.vs)?;
        let mut stages = vec![vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vs)
            .name(c"main")];
        if let Some(fs) = d.fs {
            let fs = self.shader(fs)?;
            stages.push(
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fs)
                    .name(c"main"),
            );
        }
        let bindings = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: d.stride,
            input_rate: vk::VertexInputRate::VERTEX,
        }];
        let attrs: Vec<_> = d
            .attrs
            .iter()
            .map(|&(location, format, offset)| vk::VertexInputAttributeDescription { location, binding: 0, format, offset })
            .collect();
        let vi = if d.stride > 0 {
            vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attrs)
        } else {
            vk::PipelineVertexInputStateCreateInfo::default()
        };
        let ia = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let vp = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let (bias_c, bias_s) = d.bias.unwrap_or((0.0, 0.0));
        let rs = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0)
            .depth_bias_enable(d.bias.is_some())
            .depth_bias_constant_factor(bias_c)
            .depth_bias_slope_factor(bias_s);
        let ms = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(d.samples.unwrap_or(vk::SampleCountFlags::TYPE_1));
        let ds = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(d.depth_test)
            .depth_write_enable(d.depth_write)
            .depth_compare_op(vk::CompareOp::LESS);
        let att = [vk::PipelineColorBlendAttachmentState {
            blend_enable: d.blend as u32,
            src_color_blend_factor: vk::BlendFactor::SRC_ALPHA,
            dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            color_blend_op: vk::BlendOp::ADD,
            src_alpha_blend_factor: vk::BlendFactor::SRC_ALPHA,
            dst_alpha_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            alpha_blend_op: vk::BlendOp::ADD,
            color_write_mask: vk::ColorComponentFlags::RGBA,
        }];
        let cb = vk::PipelineColorBlendStateCreateInfo::default().attachments(if d.color.is_some() { &att } else { &[] });
        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
        let colors: Vec<vk::Format> = d.color.into_iter().collect();
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&colors)
            .depth_attachment_format(d.depth.unwrap_or(vk::Format::UNDEFINED));
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vi)
            .input_assembly_state(&ia)
            .viewport_state(&vp)
            .rasterization_state(&rs)
            .multisample_state(&ms)
            .depth_stencil_state(&ds)
            .color_blend_state(&cb)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let p = unsafe { self.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }
            .map_err(|(_, e)| anyhow!("falha ao criar o pipeline gráfico: {e}"))?[0];
        self.keep(Res::Pipeline(p));
        Ok(p)
    }

    pub fn compute(&self, layout: vk::PipelineLayout, cs: &[u8]) -> Result<vk::Pipeline> {
        let m = self.shader(cs)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(m)
            .name(c"main");
        let info = vk::ComputePipelineCreateInfo::default().stage(stage).layout(layout);
        let p = unsafe { self.device.create_compute_pipelines(vk::PipelineCache::null(), &[info], None) }
            .map_err(|(_, e)| anyhow!("falha ao criar o pipeline de compute: {e}"))?[0];
        self.keep(Res::Pipeline(p));
        Ok(p)
    }

    /// Grava comandos num buffer avulso, envia e espera terminar (montagem da cena, antes de medir).
    pub fn one_shot(&self, f: impl FnOnce(vk::CommandBuffer)) -> Result<()> {
        unsafe {
            let cmd = self.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            self.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            // O que foi enviado antes (cópias, montagens) fica visível para estes comandos.
            memory_barrier(
                &self.device,
                cmd,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::MEMORY_WRITE,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
            );
            f(cmd);
            self.device.end_command_buffer(cmd)?;
            let fence = self.device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let cmds = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmds);
            let res = self
                .device
                .queue_submit(self.queue, &[submit], fence)
                .and_then(|_| self.device.wait_for_fences(&[fence], true, u64::MAX));
            self.device.destroy_fence(fence, None);
            self.device.free_command_buffers(self.pool, &cmds);
            res.context("a placa de vídeo parou de responder durante a montagem da cena")?;
        }
        Ok(())
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            for r in self.res.borrow_mut().drain(..).rev() {
                let d = &self.device;
                match r {
                    Res::Buffer(x) => d.destroy_buffer(x, None),
                    Res::Image(x) => d.destroy_image(x, None),
                    Res::View(x) => d.destroy_image_view(x, None),
                    Res::Memory(x) => d.free_memory(x, None),
                    Res::Sampler(x) => d.destroy_sampler(x, None),
                    Res::Shader(x) => d.destroy_shader_module(x, None),
                    Res::Pipeline(x) => d.destroy_pipeline(x, None),
                    Res::Layout(x) => d.destroy_pipeline_layout(x, None),
                    Res::SetLayout(x) => d.destroy_descriptor_set_layout(x, None),
                    Res::Pool(x) => d.destroy_descriptor_pool(x, None),
                    Res::Accel(x) => {
                        if let Some(a) = &self.accel {
                            a.destroy_acceleration_structure(x, None)
                        }
                    }
                }
            }
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.surface_fn.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

pub fn range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange { aspect_mask: aspect, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 }
}

#[allow(clippy::too_many_arguments)]
pub fn image_barrier(
    dev: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
    (src_stage, src_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    (dst_stage, dst_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
) {
    let b = [vk::ImageMemoryBarrier2::default()
        .image(image)
        .subresource_range(range(aspect))
        .old_layout(old)
        .new_layout(new)
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)];
    unsafe { dev.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().image_memory_barriers(&b)) };
}

pub fn memory_barrier(
    dev: &ash::Device,
    cmd: vk::CommandBuffer,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
) {
    let b = [vk::MemoryBarrier2::default()
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)];
    unsafe { dev.cmd_pipeline_barrier2(cmd, &vk::DependencyInfo::default().memory_barriers(&b)) };
}

/// Viewport e scissor do tamanho do alvo.
pub fn viewport(dev: &ash::Device, cmd: vk::CommandBuffer, w: u32, h: u32) {
    unsafe {
        dev.cmd_set_viewport(
            cmd,
            0,
            &[vk::Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32, min_depth: 0.0, max_depth: 1.0 }],
        );
        dev.cmd_set_scissor(cmd, 0, &[vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: w, height: h } }]);
    }
}

pub fn color_att(view: vk::ImageView, clear: Option<[f32; 4]>) -> vk::RenderingAttachmentInfo<'static> {
    vk::RenderingAttachmentInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(if clear.is_some() { vk::AttachmentLoadOp::CLEAR } else { vk::AttachmentLoadOp::DONT_CARE })
        .store_op(vk::AttachmentStoreOp::STORE)
        .clear_value(vk::ClearValue { color: vk::ClearColorValue { float32: clear.unwrap_or_default() } })
}

pub fn depth_att(view: vk::ImageView, store: bool) -> vk::RenderingAttachmentInfo<'static> {
    vk::RenderingAttachmentInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(if store { vk::AttachmentStoreOp::STORE } else { vk::AttachmentStoreOp::DONT_CARE })
        .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } })
}

/// Começa a renderização dinâmica num alvo de `w`×`h`.
pub fn begin_rendering(
    dev: &ash::Device,
    cmd: vk::CommandBuffer,
    w: u32,
    h: u32,
    color: Option<&vk::RenderingAttachmentInfo>,
    depth: Option<&vk::RenderingAttachmentInfo>,
) {
    let colors: Vec<vk::RenderingAttachmentInfo> = color.into_iter().copied().collect();
    let mut info = vk::RenderingInfo::default()
        .render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: w, height: h } })
        .layer_count(1)
        .color_attachments(&colors);
    if let Some(d) = depth {
        info = info.depth_attachment(d);
    }
    unsafe { dev.cmd_begin_rendering(cmd, &info) };
    viewport(dev, cmd, w, h);
}
