//! Desktop `winit` + `wgpu` application shell.
//!
//! [`Application`] is the runtime that [`engine_main!`] generates: it owns the
//! window, the GPU device/queue and the present surface, and drives a
//! continuous vsync-capped frame loop (so presentation matches the display
//! refresh, typically 60 Hz). The game only hands over a [`WindowConfig`] and
//! an [`AppDelegate`]; the delegate is called once per presented frame.
//!
//! Game crates do not write that plumbing. They implement
//! [`cubic_core::Game`], and [`GameDelegate`] adapts it to [`AppDelegate`] —
//! so `engine_main!(MyGame::new())` is the entire entry point of a game.
//!
//! [`engine_main!`]: crate::engine_main

use std::time::{Duration, Instant};

use cubic_core::Game;
use cubic_core::input::{FrameInput, InputState};
use cubic_core::render::{DrawList, Rgba};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::platform::native::NativeInput;
use crate::render2d::{QuadBatch, WgpuRenderer2d};
use crate::text::TextPipeline;

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
    /// Advance game state by `dt` wall-clock seconds. Called once per presented
    /// frame, immediately before the frame is drawn.
    ///
    /// `input` carries held state and `frame` the edges for this frame only, the
    /// same split `TickContext` uses, so a delegate's update step can move
    /// straight into a `System` once the accumulator lands.
    fn update(&mut self, dt: f32, input: &InputState, frame: &FrameInput);

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
    /// Built on the first frame that draws text: loading the system font
    /// database costs more than a frame, so text-free frames skip it.
    text: Option<TextPipeline>,
    /// Window events folded into engine input by the platform adapter.
    input: NativeInput,
    /// This frame's drained edges, kept alive because `update` borrows it
    /// alongside the delegate's own state.
    frame: FrameInput,
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
            text: None,
            input: NativeInput::new(),
            frame: FrameInput::default(),
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

        // Drain edges for the frame being presented. Held state stays in the
        // adapter and is read through the borrow below.
        self.frame = self.input.begin_frame();
        delegate.update(dt, self.input.state(), &self.frame);

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

        if self.text.is_none() && !batch.texts.is_empty() {
            let format = self
                .surface_format
                .expect("surface format set")
                .add_srgb_suffix();
            let mut text = TextPipeline::new(device, queue, format);
            text.set_resolution(queue, self.size.width.max(1), self.size.height.max(1));
            self.text = Some(text);
        } else if let Some(text) = self.text.as_mut() {
            text.set_resolution(queue, self.size.width.max(1), self.size.height.max(1));
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cubic-2d present"),
        });
        if let Err(error) = renderer.render(
            device,
            queue,
            &mut encoder,
            &view,
            &batch,
            self.text.as_mut(),
        ) {
            // The pass is dropped uncalled, so the frame still presents with
            // the clear color instead of stalling the loop.
            log::error!("drawing text failed: {error}");
        }

        queue.submit([encoder.finish()]);
        if let Some(text) = self.text.as_mut() {
            text.trim();
        }
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
        // Fold first: the adapter ignores anything it does not handle, so every
        // event reaches input without this arm needing to enumerate them.
        self.app.input.handle_event(&event);
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

/// Adapts a [`Game`] to the shell's per-frame [`AppDelegate`] callbacks.
///
/// This is the whole bridge between a game and the window: the delegate owns no
/// state beyond the game, so every frame is `Game::update` followed by
/// `Game::draw`. Hosts that cannot use [`Application`] — an editor driving a
/// game in-process, a test — can drive the `Game` directly instead; nothing in
/// the loop requires this wrapper.
pub struct GameDelegate<G> {
    game: G,
}

impl<G: Game> GameDelegate<G> {
    pub fn new(game: G) -> Self {
        Self { game }
    }

    pub fn into_game(self) -> G {
        self.game
    }

    pub fn game_mut(&mut self) -> &mut G {
        &mut self.game
    }
}

impl<G: Game> AppDelegate for GameDelegate<G> {
    fn update(&mut self, dt: f32, input: &InputState, frame: &FrameInput) {
        self.game.update(dt, input, frame);
    }

    fn draw(&mut self, list: &mut DrawList) {
        self.game.draw(list);
    }

    fn clear_color(&self) -> Rgba {
        self.game.clear_color()
    }
}

/// Own a window and run `game` until it is closed.
///
/// The one function behind [`engine_main!`]. Logging is set up here so a game
/// needs no `main` of its own; `RUST_LOG` still wins, and a game that already
/// installed a logger keeps it.
pub fn run_game<G: Game + 'static>(game: G, config: WindowConfig) -> Result<(), AppError> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // The game's own backdrop wins over the config's, so a `Game` that never
    // clears still gets the color it asked for on frames that skip `draw`.
    let config = WindowConfig {
        clear_color: game.clear_color(),
        ..config
    };
    Application::new(config).run(GameDelegate::new(game))
}

/// Write a game's entire `main`: window, loop, input pump and renderer, wired
/// around a [`cubic_core::Game`] implementation.
///
/// ```ignore
/// use cubic_render::prelude::*;
///
/// struct MyGame {
///     t: f32,
/// }
///
/// impl Game for MyGame {
///     fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
///         self.t += dt;
///     }
///
///     fn draw(&mut self, list: &mut DrawList) {
///         list.clear(Rgba::rgb(0.1, 0.1, 0.1));
///         list.fill_rect(10.0, 10.0, 100.0, 50.0, Rgba::rgb(1.0, 0.5, 0.2));
///     }
/// }
///
/// engine_main!(MyGame { t: 0.0 });
/// ```
///
/// The argument is an expression that *produces* the game — so a constructor
/// call, `MyGame::new()`, with any setup already applied. The window title
/// defaults to the crate name; call [`run_game`] directly to override the
/// title, size or backdrop.
#[macro_export]
macro_rules! engine_main {
    ($ctor:expr $(,)?) => {
        fn main() {
            $crate::run_game(
                $ctor,
                $crate::WindowConfig {
                    title: ::std::string::String::from(env!("CARGO_PKG_NAME")),
                    ..::std::default::Default::default()
                },
            )
            .expect("cubic: application ended early");
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubic_core::input::KeyCode;
    use cubic_core::render::{DrawCommand, Renderer};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the shell asked of a game, recorded behind an `Rc` so the
    /// assertions can read it after the game has been handed over.
    #[derive(Default)]
    struct Probe {
        dts: Vec<f32>,
        backdrop: Option<Rgba>,
    }

    type Shared = Rc<RefCell<Probe>>;

    struct ProbeGame(Shared);

    impl Game for ProbeGame {
        fn update(&mut self, dt: f32, _input: &InputState, _frame: &FrameInput) {
            self.0.borrow_mut().dts.push(dt);
        }

        fn draw(&mut self, list: &mut DrawList) {
            list.fill_rect(1.0, 2.0, 3.0, 4.0, Rgba::rgb(1.0, 0.0, 0.0));
        }

        fn clear_color(&self) -> Rgba {
            self.0.borrow().backdrop.unwrap_or(Rgba::rgb(0.0, 0.0, 0.0))
        }
    }

    /// Input carrying a single press, for tests that check edges arrive intact.
    fn input_with(key: KeyCode) -> (InputState, FrameInput) {
        let mut input = InputState::new();
        input.key_down(key);
        let frame = input.begin_frame();
        (input, frame)
    }

    #[test]
    fn update_is_forwarded_once_per_call_with_its_own_dt() {
        let probe: Shared = Rc::new(RefCell::new(Probe::default()));
        let mut delegate = GameDelegate::new(ProbeGame(Rc::clone(&probe)));
        let (input, frame) = input_with(KeyCode::Space);

        delegate.update(1.0 / 60.0, &input, &frame);
        delegate.update(1.0 / 30.0, &input, &frame);

        assert_eq!(probe.borrow().dts, vec![1.0 / 60.0, 1.0 / 30.0]);
    }

    #[test]
    fn draw_commands_reach_the_list_untouched() {
        let probe: Shared = Rc::new(RefCell::new(Probe::default()));
        let mut delegate = GameDelegate::new(ProbeGame(probe));
        let mut list = DrawList::new();

        delegate.draw(&mut list);

        assert!(matches!(
            list.commands.as_slice(),
            [DrawCommand::Rect {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
                ..
            }]
        ));
    }

    #[test]
    fn the_backdrop_comes_from_the_game() {
        let probe: Shared = Rc::new(RefCell::new(Probe {
            backdrop: Some(Rgba::rgb(0.1, 0.2, 0.3)),
            ..Probe::default()
        }));
        let delegate = GameDelegate::new(ProbeGame(probe));
        assert_eq!(delegate.clear_color(), Rgba::rgb(0.1, 0.2, 0.3));
    }

    /// A game that overrides nothing is still a valid game: the empty bodies
    /// must be callable, and the backdrop must be black rather than leaking
    /// whatever the window config happened to carry.
    #[test]
    fn an_empty_game_impl_is_legal_and_clears_black() {
        struct Minimal;
        impl Game for Minimal {}

        let mut delegate = GameDelegate::new(Minimal);
        assert_eq!(delegate.clear_color(), Rgba::rgb(0.0, 0.0, 0.0));

        let mut list = DrawList::new();
        delegate.update(0.016, &InputState::new(), &FrameInput::default());
        delegate.draw(&mut list);
        assert!(list.commands.is_empty());
    }

    #[test]
    fn the_game_is_reachable_through_and_out_of_the_delegate() {
        let probe: Shared = Rc::new(RefCell::new(Probe::default()));
        let mut delegate = GameDelegate::new(ProbeGame(Rc::clone(&probe)));

        delegate
            .game_mut()
            .update(0.5, &InputState::new(), &FrameInput::default());

        let game = delegate.into_game();
        assert_eq!(game.0.borrow().dts, vec![0.5]);
    }

    /// The delegate must not drain or re-time anything: a press the shell
    /// collected is the press the game sees.
    #[test]
    fn frame_edges_arrive_at_the_game_unchanged() {
        struct Waiter {
            saw_escape: Rc<RefCell<bool>>,
        }
        impl Game for Waiter {
            fn update(&mut self, _dt: f32, _input: &InputState, frame: &FrameInput) {
                *self.saw_escape.borrow_mut() = frame.pressed(KeyCode::Escape);
            }
        }

        let saw_escape = Rc::new(RefCell::new(false));
        let mut delegate = GameDelegate::new(Waiter {
            saw_escape: Rc::clone(&saw_escape),
        });
        let (input, frame) = input_with(KeyCode::Escape);

        delegate.update(0.016, &input, &frame);

        assert!(*saw_escape.borrow());
    }
}
