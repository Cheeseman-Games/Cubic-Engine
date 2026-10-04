//! Desktop `winit` + `wgpu` engine runtime.
//!
//! [`EngineApp`] is the runtime that [`engine_main!`] generates: it owns the
//! window, the GPU device/queue and the present surface, pumps platform input
//! into engine state, and drives the frame loop. The game only hands over a
//! [`WindowConfig`] and an [`AppDelegate`]; the delegate is called once per
//! presented frame.
//!
//! # Two clocks
//!
//! The loop keeps presentation and simulation apart, which is what makes
//! gameplay reproducible:
//!
//! - Presentation follows the display. The loop repaints as fast as vsync (or
//!   faster with vsync off) and redraws whatever the game's `draw` last emitted.
//! - Simulation follows [`FixedTick`]. Each frame's real elapsed time is banked,
//!   and the game's `update` runs once per whole [`FixedTick::step`] that time
//!   is worth — zero times on a 240 Hz panel, twice on a 30 Hz one, always with
//!   the same `dt`.
//!
//! So a game's own logic never sees a variable timestep, and the same input
//! replayed on different hardware produces the same match.
//!
//! Game crates do not write any of that plumbing. They implement
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
use crate::tick::{FixedTick, MAX_FRAME_SECONDS};

/// Window creation parameters for [`EngineApp::run`].
///
/// A project manifest is the usual source for one — see
/// [`WindowConfig::from_manifest`] — but a host that has its settings from
/// somewhere else (a test, a tool, a native window) can build it directly.
#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub resizable: bool,
    pub clear_color: Rgba,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "cubic".to_string(),
            width: 960.0,
            height: 540.0,
            resizable: true,
            clear_color: Rgba::rgb(0.07, 0.09, 0.13),
        }
    }
}

#[cfg(feature = "manifest")]
impl WindowConfig {
    /// The window a project manifest asks for.
    ///
    /// The manifest's logical pixel size becomes the window's initial inner
    /// size; the window manager scales it if the display cannot show it at 1:1.
    pub fn from_manifest(manifest: &cubic_core::manifest::ProjectManifest) -> Self {
        Self {
            title: manifest.window_title().to_string(),
            width: f64::from(manifest.window.width),
            height: f64::from(manifest.window.height),
            resizable: manifest.window.resizable,
            clear_color: manifest.window.clear_color,
        }
    }
}

/// Per-frame hooks the runtime calls on the game.
///
/// Keeping the boundary a trait lets the runtime grow — a renderer hook, a
/// windowless host, an editor driving a game in-process — without games changing
/// shape.
pub trait AppDelegate {
    /// Advance game state by `dt` seconds.
    ///
    /// Called once per *simulation step*, not once per presented frame, and `dt`
    /// is always [`FixedTick::step`] — never the wall-clock gap. A frame that
    /// earns no step does not call this at all, and a frame that earns several
    /// calls it several times.
    ///
    /// `input` carries held state and `frame` the edges drained for *this step*
    /// only, the same split `TickContext` uses, so a delegate's update step can
    /// move straight into a `System`.
    fn update(&mut self, dt: f32, input: &InputState, frame: &FrameInput);

    /// Emit the frame's draw commands into `list`. Called once per presented
    /// frame, after every `update` for that frame. The runtime resets the list
    /// before every call, so the delegate only ever pushes — `DrawList`
    /// (`cubic_core`) is the backend-agnostic boundary. The default emits nothing
    /// (just the clear).
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

/// The engine runtime: window, GPU device, renderer, input pump and the
/// fixed-tick clock that separates simulation from presentation.
///
/// A host builds one with a [`WindowConfig`] and runs a delegate in it; a game
/// never touches this type directly, because [`GameDelegate`] wraps a
/// [`cubic_core::Game`] into something this accepts.
pub struct EngineApp {
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
    /// Decides how many whole simulation steps each frame's real time is worth.
    tick: FixedTick,
    /// This step's drained edges, kept alive because `update` borrows it
    /// alongside the delegate's own state.
    frame: FrameInput,
    frame_list: DrawList,
    size: PhysicalSize<u32>,
    last_frame: Option<Instant>,
    fps_window_start: Option<Instant>,
    frames_in_window: u32,
}

impl EngineApp {
    /// A runtime for `config` that advances gameplay at
    /// [`DEFAULT_TICK_HZ`](crate::DEFAULT_TICK_HZ). See [`EngineApp::with_tick`]
    /// to pick another rate.
    pub fn new(config: WindowConfig) -> Self {
        Self::with_tick(config, FixedTick::default())
    }

    /// A runtime for `config` that advances gameplay on `tick`'s schedule.
    pub fn with_tick(config: WindowConfig, tick: FixedTick) -> Self {
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
            tick,
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

    /// One turn of the loop: advance simulation by however many fixed steps the
    /// frame's real time earned, then draw and present.
    fn frame<H: AppDelegate>(&mut self, delegate: &mut H) {
        let now = Instant::now();
        // The first frame has no predecessor to measure against. Crediting one
        // step gives the game a defined starting state instead of a dead frame
        // whose `dt` is zero, and it is deterministic either way — `Instant` is
        // monotonic, so this only ever happens once.
        let elapsed = self
            .last_frame
            .map(|t| (now - t).as_secs_f32())
            .unwrap_or(self.tick.step());
        self.last_frame = Some(now);

        // Simulation: `update` runs once per whole step, always with the same
        // `dt`, and each call sees edges drained for that step alone. A frame
        // that earns no step leaves the bank untouched and skips `update`
        // entirely; a frame that earns several runs `update` that many times, so
        // slow frames neither lose nor overshoot simulation time.
        advance_simulation(
            &mut self.tick,
            &mut self.input,
            &mut self.frame,
            elapsed,
            delegate,
        );

        // Presentation: one draw per frame regardless of how many steps ran, so
        // the delegate can coalesce a burst into a single picture.
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

/// Run `elapsed` seconds of real time through the fixed-step clock, calling
/// `delegate.update` once per whole step and returning how many ran.
///
/// Free-standing rather than a method so the stepping rule can be tested without
/// a window: this is the function the determinism contract lives in, and it must
/// not be verifiable only by watching pixels move.
///
/// `frame` is the scratch slot the drained edges are parked in; it is kept
/// outside so the borrow of the held input state and the frame edges can both
/// outlive a single call.
fn advance_simulation<H: AppDelegate>(
    tick: &mut FixedTick,
    input: &mut NativeInput,
    frame: &mut FrameInput,
    elapsed: f32,
    delegate: &mut H,
) -> u32 {
    let step = tick.step();
    let mut ran = 0;
    for _ in 0..tick.advance(elapsed) {
        // Drain per step, not per frame: a press that arrived between two steps
        // of the same frame is one press, so only the first step sees it.
        *frame = input.begin_frame();
        delegate.update(step, input.state(), frame);
        ran += 1;
    }
    ran
}

/// Bridges [`EngineApp`] into winit's event loop.
struct Runner<H> {
    app: EngineApp,
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
                        .with_resizable(self.app.config.resizable)
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
                self.app.frame(&mut self.delegate);
                self.app.request_redraw();
            }
            _ => {}
        }
    }
}

/// Adapts a [`Game`] to the runtime's per-step [`AppDelegate`] callbacks.
///
/// This is the whole bridge between a game and the window: the delegate owns no
/// state beyond the game, so the loop is `Game::update` once per fixed step
/// followed by one `Game::draw` per frame. Hosts that cannot use [`EngineApp`] —
/// an editor driving a game in-process, a test, a headless bench — can drive the
/// `Game` directly instead; nothing in the loop requires this wrapper.
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

/// Own a window and run `game` at [`DEFAULT_TICK_HZ`](crate::DEFAULT_TICK_HZ)
/// until the window is closed.
///
/// The one function behind [`engine_main!`](crate::engine_main!). Logging is set
/// up here so a game needs no `main` of its own; `RUST_LOG` still wins, and a
/// game that already installed a logger keeps it.
pub fn run_game<G: Game + 'static>(game: G, config: WindowConfig) -> Result<(), AppError> {
    run_game_with_tick(game, config, FixedTick::default())
}

/// Own a window and run `game` on `tick`'s schedule until the window is closed.
///
/// The seam a project manifest's tick rate plugs into: [`run_game`] is this with
/// a 60 Hz [`FixedTick`].
pub fn run_game_with_tick<G: Game + 'static>(
    game: G,
    config: WindowConfig,
    tick: FixedTick,
) -> Result<(), AppError> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // The game's own backdrop wins over the config's, so a `Game` that never
    // clears still gets the color it asked for on frames that skip `draw`.
    let config = WindowConfig {
        clear_color: game.clear_color(),
        ..config
    };
    EngineApp::with_tick(config, tick).run(GameDelegate::new(game))
}

/// The simulation clock a project manifest asks for.
///
/// Free-standing so the rate a manifest produces can be checked without opening
/// a window, and so [`run_project`] reads as "run the game the manifest
/// describes" rather than repeating the construction.
#[cfg(feature = "manifest")]
fn tick_for(manifest: &cubic_core::manifest::ProjectManifest) -> FixedTick {
    // A manifest's rate is sanitized by `FixedTick::hz`, so a project that names
    // a nonsense one runs at the default instead of failing to start.
    FixedTick::hz(manifest.game.tick_hz, MAX_FRAME_SECONDS)
}

/// Run `game` in the window its project manifest describes, ticking at the rate
/// the manifest names.
///
/// The whole of a project's entry point: a game that embeds its `game.toml`
/// calls this and has a manifest-driven window and simulation clock, without
/// naming a single field of [`WindowConfig`] or [`FixedTick`]. Everything else
/// in the manifest (features, icon, version) is about building and shipping the
/// project rather than running it.
#[cfg(feature = "manifest")]
pub fn run_project<G: Game + 'static>(
    game: G,
    manifest: &cubic_core::manifest::ProjectManifest,
) -> Result<(), AppError> {
    run_game_with_tick(
        game,
        WindowConfig::from_manifest(manifest),
        tick_for(manifest),
    )
}

/// Write a game's entire `main`: window, loop, fixed-tick clock, input pump and
/// renderer, wired around a [`cubic_core::Game`] implementation.
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
///         // `dt` is always the fixed step (1/60 s here), never the frame gap.
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
/// defaults to the crate name; call [`run_game`] or [`run_game_with_tick`]
/// directly to override the title, size, backdrop or tick rate.
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
    use crate::tick::MAX_FRAME_SECONDS;
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

    /// Everything [`advance_simulation`] needs, so each test states only the
    /// frame times it cares about.
    struct Harness {
        tick: FixedTick,
        input: NativeInput,
        frame: FrameInput,
        probe: Shared,
        delegate: GameDelegate<ProbeGame>,
    }

    impl Harness {
        fn new() -> Self {
            let probe: Shared = Rc::new(RefCell::new(Probe::default()));
            Self {
                tick: FixedTick::default(),
                input: NativeInput::new(),
                frame: FrameInput::default(),
                delegate: GameDelegate::new(ProbeGame(Rc::clone(&probe))),
                probe,
            }
        }

        /// Feed one frame of real elapsed time through the clock.
        fn frame(&mut self, elapsed: f32) -> u32 {
            advance_simulation(
                &mut self.tick,
                &mut self.input,
                &mut self.frame,
                elapsed,
                &mut self.delegate,
            )
        }

        fn dts(&self) -> Vec<f32> {
            self.probe.borrow().dts.clone()
        }
    }

    /// The load-bearing property of the whole refactor: a frame's wall-clock gap
    /// never reaches the game. Every `update` gets the same `dt`, however the
    /// time was framed.
    #[test]
    fn update_always_receives_the_fixed_step() {
        for frames in [
            vec![1.0 / 60.0; 8],
            vec![1.0 / 144.0; 19],
            vec![1.0 / 240.0 * 3.0; 5],
        ] {
            let mut harness = Harness::new();
            for &elapsed in &frames {
                harness.frame(elapsed);
            }
            let dts = harness.dts();
            assert!(!dts.is_empty(), "no step ran for {frames:?}");
            assert!(
                dts.iter().all(|&dt| dt == 1.0 / 60.0),
                "variable dt reached the game: {dts:?}"
            );
        }
    }

    /// A display faster than the tick rate idles between steps instead of
    /// interpolating a second one.
    #[test]
    fn a_frame_too_short_for_a_step_runs_no_update() {
        let mut harness = Harness::new();
        assert_eq!(harness.frame(1.0 / 240.0), 0);
        assert!(harness.dts().is_empty());
    }

    /// A display slower than the tick rate catches up by running several steps,
    /// so simulation keeps real-time pace.
    #[test]
    fn a_long_frame_runs_the_steps_it_earned() {
        let mut harness = Harness::new();
        assert_eq!(harness.frame(4.0 / 60.0), 4);
        assert_eq!(harness.dts(), vec![1.0 / 60.0; 4]);
    }

    /// The edge-drain point matters as much as the stepping: a press folded in
    /// before a multi-step frame is one press, not one per step.
    #[test]
    fn a_press_is_reported_by_exactly_one_step_of_a_multi_step_frame() {
        struct Press {
            seen: Rc<RefCell<Vec<bool>>>,
        }
        impl Game for Press {
            fn update(&mut self, _dt: f32, _input: &InputState, frame: &FrameInput) {
                self.seen.borrow_mut().push(frame.pressed(KeyCode::Space));
            }
        }

        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut delegate = GameDelegate::new(Press {
            seen: Rc::clone(&seen),
        });
        let mut input = NativeInput::new();
        input.state_mut().key_down(KeyCode::Space);
        let mut frame = FrameInput::default();
        // A 1/16 s step, so three of them is exactly representable.
        let mut tick = FixedTick::new(0.0625, MAX_FRAME_SECONDS);

        let ran = advance_simulation(&mut tick, &mut input, &mut frame, 0.1875, &mut delegate);

        assert_eq!(ran, 3);
        assert_eq!(*seen.borrow(), vec![true, false, false]);
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

    /// The whole point of the manifest layer: what `game.toml` says is what the
    /// window is, with no field copied by hand in between.
    #[cfg(feature = "manifest")]
    mod manifest_project {
        use super::*;
        use cubic_core::manifest::ProjectManifest;

        /// Parse a manifest, panicking with the reason if it does not — the
        /// shape a test wants to stay terse.
        fn manifest(text: &str) -> ProjectManifest {
            ProjectManifest::parse(text).expect("test manifest is valid")
        }

        #[test]
        fn the_manifest_becomes_the_window_it_describes() {
            let window = WindowConfig::from_manifest(&manifest(
                r##"
[game]
name = "platformer"

[window]
title = "Platformer"
width = 1280
height = 720
resizable = false
clear_color = "#204060"
"##,
            ));

            assert_eq!(window.title, "Platformer");
            assert_eq!(window.width, 1280.0);
            assert_eq!(window.height, 720.0);
            assert!(!window.resizable);
            assert!((window.clear_color.g - 64.0 / 255.0).abs() < 1e-6);
        }

        /// A project that names no title gets the one it is called, and a
        /// project that names nothing else gets a window like any other.
        #[test]
        fn an_untitled_project_opens_a_titled_window() {
            let window = WindowConfig::from_manifest(&manifest("[game]\nname = \"platformer\"\n"));

            assert_eq!(window.title, "platformer");
            assert!(window.resizable);
            assert_eq!(window.width, WindowConfig::default().width);
            assert_eq!(window.height, WindowConfig::default().height);
        }

        /// The manifest's defaults and the shell's defaults are two spellings of
        /// one decision; they must not drift apart.
        #[test]
        fn the_manifest_and_the_shell_default_to_the_same_window() {
            let default = WindowConfig::default();
            let from_manifest =
                WindowConfig::from_manifest(&manifest("[game]\nname = \"cubic\"\n"));

            assert_eq!(from_manifest.width as u32, default.width as u32);
            assert_eq!(from_manifest.height as u32, default.height as u32);
            assert_eq!(from_manifest.resizable, default.resizable);
            assert_eq!(from_manifest.clear_color, default.clear_color);
        }

        #[test]
        fn the_manifest_rate_is_the_tick_rate() {
            let tick = tick_for(&manifest("[game]\nname = \"demo\"\ntick_hz = 120\n"));
            assert!(
                (tick.rate() - 120.0).abs() < 1e-3,
                "120 Hz manifest ran at {}",
                tick.rate()
            );
            assert_eq!(tick.max_frame(), MAX_FRAME_SECONDS);
        }

        /// The contract `FixedTick` documents: a project that asks for an
        /// impossible rate runs at the default rather than refusing to start.
        #[test]
        fn a_nonsense_manifest_rate_runs_at_the_default_rate() {
            for bad in ["0", "-30", "1e40"] {
                let tick = tick_for(&manifest(&format!(
                    "[game]\nname = \"demo\"\ntick_hz = {bad}\n"
                )));
                assert!(
                    (tick.rate() - crate::DEFAULT_TICK_HZ).abs() < 1e-3,
                    "tick_hz = {bad} ran at {}",
                    tick.rate()
                );
            }
        }

        #[test]
        fn a_manifest_with_no_rate_ticks_at_the_documented_default() {
            let tick = tick_for(&manifest("[game]\nname = \"demo\"\n"));
            assert!((tick.rate() - crate::DEFAULT_TICK_HZ).abs() < 1e-3);
        }
    }
}
