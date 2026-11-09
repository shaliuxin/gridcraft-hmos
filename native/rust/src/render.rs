//! 原生窗口所有权保留在 Rust 侧（不进入上游安全 Rust workspace）。
//! 用 wgpu GL 后端在鸿蒙 NativeWindow 上直接绘制 egui，不经 eframe/winit。
use raw_window_handle::{
    DisplayHandle, HasDisplayHandle, HasWindowHandle, OhosNdkWindowHandle, WindowHandle,
};
use std::{ffi::c_void, ptr::NonNull, sync::Arc};

unsafe extern "C" {
    fn OH_NativeWindow_NativeObjectUnreference(object: *mut c_void) -> i32;
    /// 由 C++ 边界（bridge.cpp）实现：设置窗口色彩空间（P3=1 / sRGB=0）。
    fn gc_set_window_color_space(window: *mut c_void, p3: bool) -> i32;
}

/// XComponent 回调在发往 worker 前已持有原生引用；Window 拥有这一引用。
pub struct Window(pub usize);
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: 每个 Window 拥有 OnSurfaceCreated 取得的一个引用，所有借用它的 surface 销毁后才释放。
        unsafe {
            OH_NativeWindow_NativeObjectUnreference(self.0 as *mut c_void);
        }
    }
}
impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(DisplayHandle::ohos())
    }
}
impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        let ptr = NonNull::new(self.0 as *mut c_void)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        // SAFETY: 借用句柄不能超出所拥有的原生引用生命周期。
        Ok(unsafe { WindowHandle::borrow_raw(OhosNdkWindowHandle::new(ptr).into()) })
    }
}

/// 深度/模板位数。gridcraft 是纯 egui 渲染（没有 3D 视口回调、没有 overlay 管线），
/// 本 depth-stencil 附件是从 solidcraft 模板沿用的、经验证的无害路径：egui 自己的管线
/// 通过 `RendererOptions.depth_stencil_format` 声明同格式附件，渲染时一并提供，格式一致
/// 即可通过 wgpu 校验。若后续发现 GLES 上 depth 纹理开销偏大，可整段移除（renderer 侧
/// `depth_stencil_format: None`、渲染 pass 不带 depth 附件即回到标准纯 egui 路径）。
pub const DEPTH_BITS: u8 = 24;
pub const STENCIL_BITS: u8 = 8;

fn depth_format() -> wgpu::TextureFormat {
    egui_wgpu::depth_format_from_bits(DEPTH_BITS, STENCIL_BITS)
        .unwrap_or(wgpu::TextureFormat::Depth24PlusStencil8)
}

/// 清屏用的远平面深度值：24 位定点缓冲不是逆深度（深度范围 [0,1]，远平面即 1.0）。
fn far_depth() -> f32 {
    if matches!(depth_format(), wgpu::TextureFormat::Depth32Float | wgpu::TextureFormat::Depth32FloatStencil8) {
        0.0
    } else {
        1.0
    }
}

fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("GridCraft depth"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// 单 surface GLES 渲染器。子窗口（浮动文档窗口）首版未接入。
pub struct GpuRender {
    pub surface: Option<wgpu::Surface<'static>>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    pub config: wgpu::SurfaceConfiguration,
    pub adapter: wgpu::Adapter,
    pub instance: wgpu::Instance,
    pub window: Option<Arc<Window>>,
    pub color: crate::display_color::DisplayColor,
    /// 深度/模板附件：gridcraft 是纯 egui 渲染（无 3D 视口回调），此附件是从 solidcraft 模板
    /// 沿用的经验证无害路径——renderer 经 `RendererOptions.depth_stencil_format` 声明同格式，
    /// 渲染时一并提供，格式一致即通过 wgpu 校验。
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
}

impl GpuRender {
    pub fn new(window: Arc<Window>, w: u32, h: u32, request_p3: bool) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("GridCraft For HarmonyOS"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        // 未捕获的 wgpu 校验错误默认会静默丢失；接到 log 上才能在设备上看清是哪一步非法。
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            log::error!("wgpu uncaptured: {error}");
        }));
        {
            let info = adapter.get_info();
            let caps = surface.get_capabilities(&adapter);
            crate::bridge::notify(
                0,
                "diag",
                &format!(
                    "adapter name={:?} backend={:?} type={:?} driver={:?} | formats={:?} present={:?} alpha={:?}",
                    info.name,
                    info.backend,
                    info.device_type,
                    info.driver,
                    caps.formats,
                    caps.present_modes,
                    caps.alpha_modes
                ),
                &[],
            );
        }
        let mut config = surface
            .get_default_config(&adapter, w.max(1), h.max(1))
            .ok_or("no native surface configuration")?;
        // Keep the format EGL advertises: overriding it desynchronises the pipeline from the
        // NativeWindow buffer layout that the compositor actually reads.
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);
        let p3 = request_p3 && declare_color(&window, true).is_ok();
        if !p3 {
            declare_color(&window, false)?;
        }
        let color = crate::display_color::DisplayColor::new(p3)?;
        let renderer = Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                // 与上面创建的深度附件同格式：egui 自己的管线也声明这份 depth-stencil 状态。
                depth_stencil_format: Some(depth_format()),
                ..Default::default()
            },
        )));
        let (_, depth_view) = create_depth(&device, config.width, config.height);
        let depth_size = (config.width, config.height);
        Ok(Self {
            surface: Some(surface),
            device,
            queue,
            renderer,
            config,
            adapter,
            instance,
            window: Some(window),
            color,
            depth_view: Some(depth_view),
            depth_size,
        })
    }

    pub fn egui_render_state(&self) -> egui_wgpu::RenderState {
        // egui 0.37 的 RenderState 比 0.36 精简：只保留 7 个字段
        // （msaa/surface_config/device_lost 在 0.37 已并入 RendererOptions 或内部处理）。
        egui_wgpu::RenderState {
            adapter: self.adapter.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            available_adapters: vec![self.adapter.clone()],
            instance: self.instance.clone(),
            device: self.device.clone(),
            queue: self.queue.clone(),
            target_format: self.config.format,
            renderer: self.renderer.clone(),
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        self.config.width = w;
        self.config.height = h;
        if self.depth_size != (w, h) || self.depth_view.is_none() {
            let (_, view) = create_depth(&self.device, w, h);
            self.depth_view = Some(view);
            self.depth_size = (w, h);
        }
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
            if let Some(window) = &self.window {
                declare_color(window, self.color.p3)?;
            }
        }
        Ok(())
    }

    pub fn detach(&mut self) {
        self.surface = None;
        self.window = None;
    }

    pub fn bind(&mut self, window: Arc<Window>, w: u32, h: u32) -> Result<(), String> {
        let surface = self
            .instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        self.surface = Some(surface);
        self.window = Some(window);
        self.resize(w.max(1), h.max(1))
    }

    /// 把一帧 egui 画面呈现到给定 surface。
    #[allow(clippy::too_many_arguments)]
    fn draw(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &crate::display_color::DisplayColor,
        renderer: &mut egui_wgpu::Renderer,
        ctx: &egui::Context,
        surface: &wgpu::Surface<'static>,
        config: &wgpu::SurfaceConfiguration,
        depth_view: Option<&wgpu::TextureView>,
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
    ) -> Result<bool, String> {
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => return Ok(false),
            wgpu::CurrentSurfaceTexture::Lost => return Ok(false),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false)
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("native surface validation failed".into())
            }
        };
        let shape_count = shapes.len();
        let mut jobs = ctx.tessellate(shapes, pixels_per_point);
        {
            // 诊断：确认 egui 真的产出了可见图元（黑屏排查用，前几帧与长间隔抽样上报）。
            use std::sync::atomic::{AtomicUsize, Ordering};
            static FRAMES: AtomicUsize = AtomicUsize::new(0);
            let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 3 || n % 300 == 0 {
                let meshes = jobs
                    .iter()
                    .filter(|j| matches!(j.primitive, egui::epaint::Primitive::Mesh(_)))
                    .count();
                let verts = jobs
                    .iter()
                    .map(|j| match &j.primitive {
                        egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                        _ => 0,
                    })
                    .sum::<usize>();
                crate::bridge::notify(
                    0,
                    "diag",
                    &format!(
                        "frame={n} shapes={shape_count} ppp={pixels_per_point:.2} jobs={} meshes={meshes} verts={verts} cfg={}x{} tex={}x{} fmt={:?}",
                        jobs.len(),
                        config.width,
                        config.height,
                        frame.texture.width(),
                        frame.texture.height(),
                        config.format
                    ),
                    &[],
                );
            }
        }
        if color.p3 {
            for job in &mut jobs {
                if let egui::epaint::Primitive::Mesh(mesh) = &mut job.primitive {
                    for vertex in &mut mesh.vertices {
                        vertex.color = color.ui_color(vertex.color);
                    }
                }
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [config.width, config.height],
            pixels_per_point,
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut commands = renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
        let view = frame.texture.create_view(&Default::default());
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.15,
                        g: 0.15,
                        b: 0.15,
                        a: 1.,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GridCraft UI"),
                color_attachments: &attachments,
                depth_stencil_attachment: depth_view.map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(far_depth()),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0),
                            store: wgpu::StoreOp::Store,
                        }),
                    }
                }),
                ..Default::default()
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        commands.push(encoder.finish());
        queue.submit(commands);
        queue.present(frame);
        Ok(true)
    }

    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        mut output: egui::FullOutput,
    ) -> Result<bool, String> {
        // 先消费 egui 的纹理增量（含释放），避免 unwind 触发调试用 Drop 断言。
        let mut texture_updates = std::mem::take(&mut output.textures_delta.set);
        let free = std::mem::take(&mut output.textures_delta.free);
        output.textures_delta.clear();
        let mut renderer = self.renderer.write();
        for (id, deltas) in &mut texture_updates {
            for delta in deltas {
                renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        for id in free {
            renderer.free_texture(&id);
        }
        let Some(surface) = &self.surface else {
            return Ok(false);
        };
        let painted = Self::draw(
            &self.device,
            &self.queue,
            &self.color,
            &mut renderer,
            ctx,
            surface,
            &self.config,
            self.depth_view.as_ref(),
            output.shapes,
            output.pixels_per_point,
        )?;
        Ok(painted)
    }
}

/// 拥有原生引用，独立于 GPU 生命周期。
pub struct Render {
    pub width: u32,
    pub height: u32,
    window: Option<Arc<Window>>,
    gpu: Option<GpuRender>,
    request_p3: bool,
}

#[allow(dead_code)]
impl Render {
    pub fn new(request_p3: bool) -> Result<Self, String> {
        Ok(Self {
            width: 0,
            height: 0,
            window: None,
            gpu: None,
            request_p3,
        })
    }

    pub fn attached(&self) -> bool {
        self.window.is_some()
    }

    pub fn color(&self) -> &crate::display_color::DisplayColor {
        self.gpu
            .as_ref()
            .map(|g| &g.color)
            .unwrap_or(&crate::display_color::DisplayColor { p3: false })
    }

    pub fn fault(&self) -> Option<String> {
        None
    }

    pub fn halt(&mut self) {
        if let Some(gpu) = self.gpu.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(gpu)));
        }
    }

    pub fn egui_render_state(&self) -> Option<egui_wgpu::RenderState> {
        self.gpu.as_ref().map(GpuRender::egui_render_state)
    }

    pub fn bind(&mut self, window: Window, w: u32, h: u32) -> Result<(), String> {
        self.window = Some(Arc::new(window));
        self.width = w;
        self.height = h;
        let window = self.window.clone().ok_or("native window unavailable")?;
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.bind(window, w, h)?;
        } else {
            self.gpu = Some(GpuRender::new(window, w, h, self.request_p3)?);
        }
        Ok(())
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String> {
        self.width = w;
        self.height = h;
        if w == 0 || h == 0 {
            return Ok(());
        }
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.resize(w, h)
        } else {
            Ok(())
        }
    }

    pub fn detach(&mut self) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.detach();
        }
        self.window = None;
    }

    pub fn paint(&mut self, ctx: &egui::Context, output: egui::FullOutput) -> Result<bool, String> {
        if self.width == 0 || self.height == 0 {
            return Ok(false);
        }
        let gpu = self.gpu.as_mut().ok_or("native GPU renderer unavailable")?;
        gpu.paint(ctx, output)
    }
}

fn declare_color(window: &Window, p3: bool) -> Result<(), String> {
    // SAFETY: Window 拥有有效引用；所有色彩声明与 EGL 操作在 render worker 上串行执行。
    let result = unsafe { gc_set_window_color_space(window.0 as *mut c_void, p3) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!("native output color space rejected: {result}"))
    }
}
