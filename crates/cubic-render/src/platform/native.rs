//! Desktop (`winit`) input adapter.
//!
//! Translates `WindowEvent`s into `cubic_core::input::InputState`. Nothing here
//! reaches past this module: callers see only `InputState` / `FrameInput`, so
//! the winit types can change without touching gameplay.

use cubic_core::input::{FrameInput, InputState, KeyCode, MouseButton};
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode as WinitKeyCode, PhysicalKey};

use crate::platform::PIXELS_PER_SCROLL_LINE;

/// Physical modifier keys currently held.
///
/// `cubic_core` models one `Shift` / `Control` / `Alt`, but a keyboard reports
/// left and right separately. Without this, holding both shifts and releasing
/// one would report the modifier as released while it is still down.
#[derive(Default)]
struct HeldModifiers {
    shift: u8,
    control: u8,
    alt: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Modifier {
    Shift,
    Control,
    Alt,
}

/// Owns the window's `InputState` and folds events into it.
pub struct NativeInput {
    state: InputState,
    modifiers: HeldModifiers,
}

impl Default for NativeInput {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeInput {
    pub fn new() -> Self {
        Self {
            state: InputState::new(),
            modifiers: HeldModifiers::default(),
        }
    }

    /// Accumulated input, for held-state queries.
    pub fn state(&self) -> &InputState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut InputState {
        &mut self.state
    }

    /// Drain this frame's edges. Call once per simulation tick.
    pub fn begin_frame(&mut self) -> FrameInput {
        self.state.begin_frame()
    }

    /// Fold one window event into the state. Unhandled events are ignored, so
    /// the caller can forward everything.
    pub fn handle_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                self.on_key(event.state, event.physical_key)
            }
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_button(*state, *button),
            WindowEvent::CursorMoved { position, .. } => {
                self.state.mouse_move(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            _ => {}
        }
    }

    fn on_key(&mut self, state: ElementState, key: PhysicalKey) {
        let PhysicalKey::Code(code) = key else {
            return;
        };
        if let Some(modifier) = modifier_of(code) {
            self.set_modifier(modifier, state, modifier_slot(code));
            return;
        }
        let Some(code) = key_code_of(code) else {
            return;
        };
        match state {
            ElementState::Pressed => self.state.key_down(code),
            ElementState::Released => self.state.key_up(code),
        }
    }

    /// Track a modifier by physical key, reporting the edge only when the first
    /// one goes down or the last one comes up.
    fn set_modifier(&mut self, modifier: Modifier, state: ElementState, slot: u8) {
        let held = match modifier {
            Modifier::Shift => &mut self.modifiers.shift,
            Modifier::Control => &mut self.modifiers.control,
            Modifier::Alt => &mut self.modifiers.alt,
        };
        let was_held = *held != 0;
        match state {
            ElementState::Pressed => *held |= slot,
            ElementState::Released => *held &= !slot,
        }
        let is_held = *held != 0;
        if was_held == is_held {
            return;
        }
        let code = match modifier {
            Modifier::Shift => KeyCode::Shift,
            Modifier::Control => KeyCode::Control,
            Modifier::Alt => KeyCode::Alt,
        };
        if is_held {
            self.state.key_down(code);
        } else {
            self.state.key_up(code);
        }
    }

    fn on_mouse_button(&mut self, state: ElementState, button: WinitMouseButton) {
        // Extra buttons have no `cubic_core` equivalent yet; dropping them
        // keeps the mapping total instead of inventing a variant per device.
        let Some(button) = mouse_button_of(button) else {
            return;
        };
        match state {
            ElementState::Pressed => self.state.mouse_down(button),
            ElementState::Released => self.state.mouse_up(button),
        }
    }

    fn on_wheel(&mut self, delta: &MouseScrollDelta) {
        // `PixelDelta` carries f64 positions, so scale in f64 and narrow once
        // rather than widening the line delta that is already f32.
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (*x, *y),
            // A trackpad reports pixels; scale to lines so both feel the same.
            MouseScrollDelta::PixelDelta(delta) => (
                (delta.x / PIXELS_PER_SCROLL_LINE as f64) as f32,
                (delta.y / PIXELS_PER_SCROLL_LINE as f64) as f32,
            ),
        };
        self.state.mouse_scroll(x, y);
    }
}

fn modifier_slot(code: WinitKeyCode) -> u8 {
    match code {
        WinitKeyCode::ShiftRight => 2,
        WinitKeyCode::ControlRight => 2,
        WinitKeyCode::AltRight => 2,
        _ => 1,
    }
}

fn modifier_of(code: WinitKeyCode) -> Option<Modifier> {
    match code {
        WinitKeyCode::ShiftLeft | WinitKeyCode::ShiftRight => Some(Modifier::Shift),
        WinitKeyCode::ControlLeft | WinitKeyCode::ControlRight => Some(Modifier::Control),
        WinitKeyCode::AltLeft | WinitKeyCode::AltRight => Some(Modifier::Alt),
        _ => None,
    }
}

fn mouse_button_of(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        // Back / forward / thumb buttons have no engine equivalent yet;
        // dropping them keeps the mapping total instead of inventing a variant
        // per device.
        WinitMouseButton::Back | WinitMouseButton::Forward | WinitMouseButton::Other(_) => None,
    }
}

/// Map a winit *physical* key onto the engine's `KeyCode`.
///
/// Physical rather than logical, so the WASD cluster keeps its meaning on
/// AZERTY and friends. Keys with no gameplay meaning map to `None`.
fn key_code_of(code: WinitKeyCode) -> Option<KeyCode> {
    let key = match code {
        WinitKeyCode::KeyA => KeyCode::A,
        WinitKeyCode::KeyB => KeyCode::B,
        WinitKeyCode::KeyC => KeyCode::C,
        WinitKeyCode::KeyD => KeyCode::D,
        WinitKeyCode::KeyE => KeyCode::E,
        WinitKeyCode::KeyF => KeyCode::F,
        WinitKeyCode::KeyG => KeyCode::G,
        WinitKeyCode::KeyH => KeyCode::H,
        WinitKeyCode::KeyI => KeyCode::I,
        WinitKeyCode::KeyJ => KeyCode::J,
        WinitKeyCode::KeyK => KeyCode::K,
        WinitKeyCode::KeyL => KeyCode::L,
        WinitKeyCode::KeyM => KeyCode::M,
        WinitKeyCode::KeyN => KeyCode::N,
        WinitKeyCode::KeyO => KeyCode::O,
        WinitKeyCode::KeyP => KeyCode::P,
        WinitKeyCode::KeyQ => KeyCode::Q,
        WinitKeyCode::KeyR => KeyCode::R,
        WinitKeyCode::KeyS => KeyCode::S,
        WinitKeyCode::KeyT => KeyCode::T,
        WinitKeyCode::KeyU => KeyCode::U,
        WinitKeyCode::KeyV => KeyCode::V,
        WinitKeyCode::KeyW => KeyCode::W,
        WinitKeyCode::KeyX => KeyCode::X,
        WinitKeyCode::KeyY => KeyCode::Y,
        WinitKeyCode::KeyZ => KeyCode::Z,
        WinitKeyCode::Digit0 => KeyCode::Digit0,
        WinitKeyCode::Digit1 => KeyCode::Digit1,
        WinitKeyCode::Digit2 => KeyCode::Digit2,
        WinitKeyCode::Digit3 => KeyCode::Digit3,
        WinitKeyCode::Digit4 => KeyCode::Digit4,
        WinitKeyCode::Digit5 => KeyCode::Digit5,
        WinitKeyCode::Digit6 => KeyCode::Digit6,
        WinitKeyCode::Digit7 => KeyCode::Digit7,
        WinitKeyCode::Digit8 => KeyCode::Digit8,
        WinitKeyCode::Digit9 => KeyCode::Digit9,
        WinitKeyCode::Space => KeyCode::Space,
        WinitKeyCode::ArrowLeft => KeyCode::Left,
        WinitKeyCode::ArrowRight => KeyCode::Right,
        WinitKeyCode::ArrowUp => KeyCode::Up,
        WinitKeyCode::ArrowDown => KeyCode::Down,
        WinitKeyCode::BracketLeft => KeyCode::BracketLeft,
        WinitKeyCode::BracketRight => KeyCode::BracketRight,
        WinitKeyCode::Minus => KeyCode::Minus,
        WinitKeyCode::Equal => KeyCode::Equal,
        WinitKeyCode::Comma => KeyCode::Comma,
        WinitKeyCode::Period => KeyCode::Period,
        WinitKeyCode::Slash => KeyCode::Slash,
        WinitKeyCode::Backslash => KeyCode::Backslash,
        WinitKeyCode::Semicolon => KeyCode::Semicolon,
        WinitKeyCode::Quote => KeyCode::Quote,
        WinitKeyCode::Backquote => KeyCode::Backquote,
        WinitKeyCode::Enter | WinitKeyCode::NumpadEnter => KeyCode::Enter,
        WinitKeyCode::Escape => KeyCode::Escape,
        WinitKeyCode::Tab => KeyCode::Tab,
        WinitKeyCode::Backspace => KeyCode::Backspace,
        WinitKeyCode::Delete => KeyCode::Delete,
        WinitKeyCode::Insert => KeyCode::Insert,
        WinitKeyCode::Home => KeyCode::Home,
        WinitKeyCode::End => KeyCode::End,
        WinitKeyCode::PageUp => KeyCode::PageUp,
        WinitKeyCode::PageDown => KeyCode::PageDown,
        WinitKeyCode::F1 => KeyCode::F1,
        WinitKeyCode::F2 => KeyCode::F2,
        WinitKeyCode::F3 => KeyCode::F3,
        WinitKeyCode::F4 => KeyCode::F4,
        WinitKeyCode::F5 => KeyCode::F5,
        WinitKeyCode::F6 => KeyCode::F6,
        WinitKeyCode::F7 => KeyCode::F7,
        WinitKeyCode::F8 => KeyCode::F8,
        WinitKeyCode::F9 => KeyCode::F9,
        WinitKeyCode::F10 => KeyCode::F10,
        WinitKeyCode::F11 => KeyCode::F11,
        WinitKeyCode::F12 => KeyCode::F12,
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubic_core::math::Vec2;
    use winit::event::DeviceId;

    #[test]
    fn the_movement_cluster_maps_from_physical_keys() {
        assert_eq!(key_code_of(WinitKeyCode::KeyW), Some(KeyCode::W));
        assert_eq!(key_code_of(WinitKeyCode::KeyA), Some(KeyCode::A));
        assert_eq!(key_code_of(WinitKeyCode::KeyS), Some(KeyCode::S));
        assert_eq!(key_code_of(WinitKeyCode::KeyD), Some(KeyCode::D));
        assert_eq!(key_code_of(WinitKeyCode::ArrowUp), Some(KeyCode::Up));
    }

    #[test]
    fn every_mapped_key_has_a_name_the_web_adapter_can_parse() {
        // The two adapters meet at `KeyCode`; a key that only the native side
        // can produce would be unreachable in the browser build.
        for code in [
            WinitKeyCode::KeyA,
            WinitKeyCode::KeyZ,
            WinitKeyCode::Digit0,
            WinitKeyCode::Digit9,
            WinitKeyCode::Space,
            WinitKeyCode::Comma,
            WinitKeyCode::Period,
            WinitKeyCode::Slash,
            WinitKeyCode::Backslash,
            WinitKeyCode::F1,
            WinitKeyCode::F12,
            WinitKeyCode::PageUp,
            WinitKeyCode::Home,
            WinitKeyCode::Insert,
        ] {
            let key = key_code_of(code).expect("key should map");
            assert_eq!(
                KeyCode::from_name(key.name()),
                Some(key),
                "{key:?} is unreachable from the web adapter"
            );
        }
    }

    #[test]
    fn unmapped_and_modifier_keys_are_excluded_from_the_plain_table() {
        assert_eq!(key_code_of(WinitKeyCode::ShiftLeft), None);
        assert_eq!(key_code_of(WinitKeyCode::CapsLock), None);
        assert_eq!(key_code_of(WinitKeyCode::ScrollLock), None);
    }

    #[test]
    fn the_three_buttons_map_and_extras_do_not() {
        assert_eq!(
            mouse_button_of(WinitMouseButton::Left),
            Some(MouseButton::Left)
        );
        assert_eq!(
            mouse_button_of(WinitMouseButton::Right),
            Some(MouseButton::Right)
        );
        assert_eq!(
            mouse_button_of(WinitMouseButton::Middle),
            Some(MouseButton::Middle)
        );
        assert_eq!(mouse_button_of(WinitMouseButton::Back), None);
        assert_eq!(mouse_button_of(WinitMouseButton::Forward), None);
        assert_eq!(mouse_button_of(WinitMouseButton::Other(8)), None);
    }

    #[test]
    fn modifiers_are_recognized_on_both_sides() {
        assert_eq!(modifier_of(WinitKeyCode::ShiftLeft), Some(Modifier::Shift));
        assert_eq!(modifier_of(WinitKeyCode::ShiftRight), Some(Modifier::Shift));
        assert_eq!(
            modifier_of(WinitKeyCode::ControlLeft),
            Some(Modifier::Control)
        );
        assert_eq!(modifier_of(WinitKeyCode::AltRight), Some(Modifier::Alt));
        assert_eq!(modifier_of(WinitKeyCode::KeyA), None);
    }

    #[test]
    fn left_and_right_modifiers_get_distinct_slots() {
        assert_ne!(
            modifier_slot(WinitKeyCode::ShiftLeft),
            modifier_slot(WinitKeyCode::ShiftRight)
        );
        assert_ne!(
            modifier_slot(WinitKeyCode::ControlLeft),
            modifier_slot(WinitKeyCode::ControlRight)
        );
    }

    /// Holding both shifts and letting go of one must leave the modifier down.
    #[test]
    fn a_modifier_stays_held_while_any_physical_key_remains() {
        let mut input = NativeInput::new();
        input.set_modifier(Modifier::Shift, ElementState::Pressed, 1);
        input.set_modifier(Modifier::Shift, ElementState::Pressed, 2);
        assert!(input.state().is_down(KeyCode::Shift));

        input.set_modifier(Modifier::Shift, ElementState::Released, 1);
        assert!(
            input.state().is_down(KeyCode::Shift),
            "one shift is still held"
        );

        input.set_modifier(Modifier::Shift, ElementState::Released, 2);
        assert!(!input.state().is_down(KeyCode::Shift));
    }

    #[test]
    fn a_modifier_reports_one_press_and_one_release_edge() {
        let mut input = NativeInput::new();
        input.set_modifier(Modifier::Control, ElementState::Pressed, 1);
        input.set_modifier(Modifier::Control, ElementState::Pressed, 2);
        let frame = input.begin_frame();
        assert!(frame.pressed(KeyCode::Control));

        input.set_modifier(Modifier::Control, ElementState::Released, 1);
        let frame = input.begin_frame();
        assert!(!frame.released(KeyCode::Control));

        input.set_modifier(Modifier::Control, ElementState::Released, 2);
        let frame = input.begin_frame();
        assert!(frame.released(KeyCode::Control));
    }

    #[test]
    fn a_duplicate_modifier_press_does_not_re_edge() {
        let mut input = NativeInput::new();
        input.set_modifier(Modifier::Alt, ElementState::Pressed, 1);
        input.set_modifier(Modifier::Alt, ElementState::Pressed, 1);
        assert!(input.begin_frame().pressed(KeyCode::Alt));
        assert!(!input.begin_frame().pressed(KeyCode::Alt));
    }

    #[test]
    fn a_release_with_no_prior_press_reports_nothing() {
        let mut input = NativeInput::new();
        input.set_modifier(Modifier::Shift, ElementState::Released, 1);
        assert!(!input.begin_frame().released(KeyCode::Shift));
    }

    #[test]
    fn a_line_wheel_delta_passes_through_unscaled() {
        let mut input = NativeInput::new();
        input.on_wheel(&MouseScrollDelta::LineDelta(0.0, 3.0));
        assert_eq!(input.begin_frame().mouse_scroll_y, 3.0);
    }

    #[test]
    fn a_pixel_wheel_delta_is_scaled_to_lines() {
        let mut input = NativeInput::new();
        input.on_wheel(&MouseScrollDelta::PixelDelta(
            winit::dpi::PhysicalPosition::new(0.0, PIXELS_PER_SCROLL_LINE as f64 * 2.0),
        ));
        assert!((input.begin_frame().mouse_scroll_y - 2.0).abs() < 1e-5);
    }

    #[test]
    fn a_horizontal_wheel_delta_is_kept() {
        let mut input = NativeInput::new();
        input.on_wheel(&MouseScrollDelta::PixelDelta(
            winit::dpi::PhysicalPosition::new(-PIXELS_PER_SCROLL_LINE as f64, 0.0),
        ));
        assert!((input.begin_frame().mouse_scroll_x + 1.0).abs() < 1e-5);
    }

    #[test]
    fn unhandled_window_events_are_ignored() {
        let mut input = NativeInput::new();
        input.handle_event(&WindowEvent::Focused(true));
        input.handle_event(&WindowEvent::CursorLeft {
            device_id: DeviceId::dummy(),
        });
        input.handle_event(&WindowEvent::Occluded(true));
        input.handle_event(&WindowEvent::Destroyed);
        let frame = input.begin_frame();
        assert!(frame.pressed.is_empty());
        assert!(frame.mouse_pressed.is_empty());
        assert_eq!(frame.mouse_scroll_delta(), Vec2::ZERO);
    }

    #[test]
    fn cursor_motion_reaches_the_state() {
        let mut input = NativeInput::new();
        input.handle_event(&WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: winit::dpi::PhysicalPosition::new(30.0, 40.0),
        });
        // The first move only sets the origin, so read position rather than
        // delta here; `mouse_motion_is_reported_after_the_first_move` covers
        // the delta path.
        assert_eq!(input.begin_frame().mouse_position(), Vec2::new(30.0, 40.0));
    }

    #[test]
    fn mouse_motion_is_reported_after_the_first_move() {
        let mut input = NativeInput::new();
        input.state_mut().mouse_move(10.0, 10.0);
        input.begin_frame();
        input.handle_event(&WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: winit::dpi::PhysicalPosition::new(14.0, 7.0),
        });
        let frame = input.begin_frame();
        assert_eq!(frame.mouse_position(), Vec2::new(14.0, 7.0));
        assert_eq!(frame.mouse_delta(), Vec2::new(4.0, -3.0));
    }

    #[test]
    fn keys_reach_the_frame_through_on_key() {
        let mut input = NativeInput::new();
        input.on_key(ElementState::Pressed, PhysicalKey::Code(WinitKeyCode::KeyW));
        let frame = input.begin_frame();
        assert!(frame.pressed(KeyCode::W));
        assert!(input.state().is_down(KeyCode::W));

        input.on_key(
            ElementState::Released,
            PhysicalKey::Code(WinitKeyCode::KeyW),
        );
        assert!(input.begin_frame().released(KeyCode::W));
        assert!(!input.state().is_down(KeyCode::W));
    }

    /// Only physical keys are honored, so a character-only event (an IME
    /// commit, say) does not fabricate a keypress.
    #[test]
    fn a_non_physical_key_is_ignored() {
        let mut input = NativeInput::new();
        input.on_key(
            ElementState::Pressed,
            PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
        );
        assert!(input.begin_frame().pressed.is_empty());
    }

    /// The engine models one Shift but the keyboard reports left and right.
    #[test]
    fn both_shifts_route_to_the_same_engine_key() {
        let mut input = NativeInput::new();
        input.on_key(
            ElementState::Pressed,
            PhysicalKey::Code(WinitKeyCode::ShiftLeft),
        );
        input.on_key(
            ElementState::Pressed,
            PhysicalKey::Code(WinitKeyCode::ShiftRight),
        );
        assert!(input.state().is_down(KeyCode::Shift));
        assert!(input.begin_frame().pressed(KeyCode::Shift));
    }

    /// The end-to-end path through `handle_event` for the events the test
    /// harness can construct.
    #[test]
    fn mouse_buttons_reach_the_frame_through_handle_event() {
        let mut input = NativeInput::new();
        input.handle_event(&WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Pressed,
            button: WinitMouseButton::Left,
        });
        assert!(input.begin_frame().mouse_pressed(MouseButton::Left));
        assert!(input.state().is_mouse_down(MouseButton::Left));
    }

    #[test]
    fn the_wheel_reaches_the_frame_through_handle_event() {
        let mut input = NativeInput::new();
        input.handle_event(&WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::LineDelta(0.0, 2.0),
            phase: winit::event::TouchPhase::Ended,
        });
        assert!((input.begin_frame().mouse_scroll_y - 2.0).abs() < 1e-5);
    }
}
