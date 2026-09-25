//! Desktop `winit` + `wgpu` application shell.
//!
//! [`Application`] is the seed of the runtime that `engine_main!` will later
//! generate: it owns the window, the GPU device/queue and the present surface,
//! and drives a continuous vsync-capped frame loop (so presentation matches
//! the display refresh, typically 60 Hz). The game only hands over a
//! [`WindowConfig`] and an [`AppDelegate`]; the delegate is called once per
//! presented frame.

use std::time::{Duration, Instant};

use cubic_core::render::{DrawList, Rgba};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::render2d::{QuadBatch, WgpuRenderer2d};

/// Window creation parameters for [`Application::run`].
#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub clear_color: Rgba,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "cubic".to_string(),
            width: 960.0,
            height: 540.0,
            clear_color: Rgba::rgb(0.07, 0.09, 0.13),
        }
    }
}

/// Per-frame hooks the shell calls on the game.
///
/// Keeping the boundary a trait lets the runtime grow (a renderer hook, then
/// a fixed-tick accumulator) without games changing shape.
pub trait AppDelegate {
    /// Advance game state by `dt` wall-clock seconds. Called once per
    /// presented frame, immediately before the frame is drawn.
    fn update(&mut self, _dt: f32) {}

    /// Emit the frame's draw commands into `list`. Called once per presented
    /// frame after `update`. The shell resets the list before every call, so
    /// the delegate only ever pushes — `DrawList` (`cubic_core`) is the
    /// backend-agnostic boundary. The default emits nothing (just the clear).
    fn draw(&mut self, _list: &mut DrawList) {}

    /// Backbuffer fill color for frames whose `draw` pushed no `Clear`
    /// command of its own.
    fn clear_color(&self) -> Rgba;
}

/// Fatal setup/loop errors that abort the application.
#[derive(Debug)]
pub enum AppError {
    EventLoop(winit::error::EventLoopError),
    Run(winit::error::EventLoopError),
    Window(winit::error::OsError),
    Surface(wgpu::CreateSurfaceError),
    Adapter(wgpu::RequestAdapterError),
    Device(wgpu::RequestDeviceError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EventLoop(e) => write!(f, "creating the event loop failed: {e}"),
            Self::Run(e) => write!(f, "event loop terminated with an error: {e}"),
            Self::Window(e) => write!(f, "creating the window failed: {e}"),
            Self::Surface(e) => write!(f, "creating the wgpu surface failed: {e}"),
            Self::Adapter(e) => write!(f, "no compatible wgpu adapter found: {e}"),
            Self::Device(e) => write!(f, "wgpu device request failed: {e}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EventLoop(e) => Some(e),
            Self::Run(e) => Some(e),
            Self::Window(e) => Some(e),
            Self::Surface(e) => Some(e),
            Self::Adapter(e) => Some(e),
            Self::Device(e) => Some(e),
        }
    }
}

/// The present surface plus the GPU device/queue it submits to.
pub struct Application {
    config: WindowConfig,
    window: Option<std::sync::Arc<Window>>,
    instance: Option<wgpu::Instance>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    surface: Option<wgpu::Surface<'static>>,
    surface_format: Option<wgpu::TextureFormat>,
    renderer: Option<WgpuRenderer2d>,
    frame_list: DrawList,
    size: PhysicalSize<u32>,
    last_frame: Option<Instant>,
    fps_window_start: Option<Instant>,
    frames_in_window: u32,
}

impl Application {
    pub fn new(config: WindowConfig) -> Self {
        Self {
            config,
            window: None,
            instance: None,
            device: None,
            queue: None,
            surface: None,
            surface_format: None,
            renderer: None,
            frame_list: DrawList::new(),
            size: PhysicalSize::new(0, 0),
            last_frame: None,
            fps_window_start: None,
            frames_in_window: 0,
        }
    }

    /// Block until the window is closed. `delegate` owns the game.
    pub fn run<H: AppDelegate + 'static>(self, delegate: H) -> Result<(), AppError> {
        let event_loop = EventLoop::new().map_err(AppError::EventLoop)?;
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut runner = Runner {
            app: self,
            delegate,
            fatal: None,
        };
        event_loop.run_app(&mut runner).map_err(AppError::Run)?;
        match runner.fatal {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn window(&self) -> &Window {
        self.window.as_ref().expect("window initialized before use")
    }

    fn request_redraw(&self) {
        self.window().request_redraw();
    }

    /// (Re)apply the surface size/format/vsync configuration.
    fn configure_surface(&mut self) {
        let device = self.device.as_ref().expect("device initialized");
        let surface = self.surface.as_ref().expect("surface initialized");
        let format = self.surface_format.expect("surface format selected");
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            // Request an sRGB-view variant of the swapchain format so the
            // clear op is gamma-correct on presentation.
            view_formats: vec![format.add_srgb_suffix()],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: self.size.width.max(1),
            height: self.size.height.max(1),
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::AutoVsync,
        };
        surface.configure(device, &config);
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        self.configure_surface();
    }

    fn render<H: AppDelegate>(&mut self, delegate: &mut H) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| (now - t).as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.last_frame = Some(now);
        delegate.update(dt);

        self.frame_list.reset();
        delegate.draw(&mut self.frame_list);

        let surface_texture = match self.acquire_frame() {
            Some(texture) => texture,
            None => return,
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(
                    self.surface_format
                        .expect("surface format set")
                        .add_srgb_suffix(),
                ),
                ..Default::default()
            });

        let device = self.device.as_ref().expect("device initialized");
        let queue = self.queue.as_ref().expect("queue initialized");
        let renderer = self.renderer.as_mut().expect("renderer initialized");

        let size = [self.size.width as f32, self.size.height as f32];
        let batch = QuadBatch::from_list(&self.frame_list, size, delegate.clear_color());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cubic-2d present"),
        });
        renderer.render(device, queue, &mut encoder, &view, &batch);

        queue.submit([encoder.finish()]);
        self.window().pre_present_notify();
        queue.present(surface_texture);

        self.report_fps(now);
    }

    /// Acquire the next backbuffer, reconfiguring the surface on the error
    /// paths that require it. Returns `None` when the frame was skipped.
    fn acquire_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        let surface = self.surface.as_ref().expect("surface initialized");
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => Some(texture),
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => None,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                drop(texture);
                self.configure_surface();
                None
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.configure_surface();
                None
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                unreachable!("validation failures surface as panics")
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                let instance = self.instance.as_ref().expect("instance initialized");
                let window = self.window.clone().expect("window initialized");
                self.surface = Some(
                    instance
                        .create_surface(window)
                        .expect("recreating a lost surface"),
                );
                self.configure_surface();
                None
            }
        }
    }

    fn report_fps(&mut self, now: Instant) {
        const PERIOD: Duration = Duration::from_secs(1);
        self.frames_in_window += 1;
        match self.fps_window_start {
            None => self.fps_window_start = Some(now),
            Some(start) => {
                let elapsed = now.duration_since(start);
                if elapsed >= PERIOD {
                    let fps = self.frames_in_window as f32 / elapsed.as_secs_f32();
                    log::info!("presented at {fps:.0} fps");
                    self.fps_window_start = Some(now);
                    self.frames_in_window = 0;
                }
            }
        }
    }
}

/// Bridges [`Application`] into winit's event loop.
struct Runner<H> {
    app: Application,
    delegate: H,
    fatal: Option<AppError>,
}

impl<H: AppDelegate> Runner<H> {
    /// Adopt the window + surface + device. Runs once per process resume.
    async fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<(), AppError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let window = std::sync::Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(self.app.config.title.clone())
                        .with_inner_size(LogicalSize::new(
                            self.app.config.width,
                            self.app.config.height,
                        )),
                )
                .map_err(AppError::Window)?,
        );

        let size = window.inner_size();
        let surface = instance
            .create_surface(window.clone())
            .map_err(AppError::Surface)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                apply_limit_buckets: false,
            })
            .await
            .map_err(AppError::Adapter)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(AppError::Device)?;
        let surface_format = surface.get_capabilities(&adapter).formats[0];

        log::info!(
            "adapter: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        let render_format = surface_format.add_srgb_suffix();
        let renderer = WgpuRenderer2d::new(&device, render_format);

        self.app.window = Some(window);
        self.app.instance = Some(instance);
        self.app.device = Some(device);
        self.app.queue = Some(queue);
        self.app.surface = Some(surface);
        self.app.surface_format = Some(surface_format);
        self.app.renderer = Some(renderer);
        self.app.size = size;
        self.app.configure_surface();
        self.app.request_redraw();
        Ok(())
    }
}

impl<H: AppDelegate> ApplicationHandler for Runner<H> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Platforms may emit this more than once; the GPU state is one-shot.
        if self.app.device.is_some() {
            self.app.request_redraw();
            return;
        }
        match pollster::block_on(self.init(event_loop)) {
            Ok(()) => {}
            Err(error) => {
                self.fatal = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                // A 0x0 surface (minimized) can't be configured; skip it.
                if size.width > 0 && size.height > 0 {
                    self.app.resize(size);
                }
            }
            WindowEvent::RedrawRequested => {
                self.app.render(&mut self.delegate);
                self.app.request_redraw();
            }
            _ => {}
        }
    }
}
