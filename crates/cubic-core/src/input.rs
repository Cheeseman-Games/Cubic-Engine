//! Input state: keys, pointer, and analogue axes.
//!
//! Platform adapters feed raw events in through the `key_*` / `mouse_*`
//! methods; gameplay reads held state from [`InputState`] and per-frame edges
//! from [`FrameInput`] via [`InputState::begin_frame`]. Gameplay code never
//! sees a `winit` or DOM type.

use std::collections::HashSet;

use crate::math::Vec2;

/// Keys the game understands. The platform layer maps raw events into these.
///
/// Named for *positions* rather than the characters printed on them, so a
/// movement cluster keeps working on non-QWERTY layouts when the adapter
/// reports physical keys.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Space,
    Left,
    Right,
    Up,
    Down,
    BracketLeft,
    BracketRight,
    Minus,
    Equal,
    Comma,
    Period,
    Slash,
    Backslash,
    Semicolon,
    Quote,
    Backquote,
    Enter,
    Escape,
    Tab,
    Shift,
    Control,
    Alt,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl KeyCode {
    /// A stable name, as a DOM `KeyboardEvent.key` reports it.
    ///
    /// Exhaustive on purpose: adding a variant without naming it fails to
    /// compile, which keeps this table and `from_name` in step.
    pub fn name(self) -> &'static str {
        match self {
            KeyCode::A => "a",
            KeyCode::B => "b",
            KeyCode::C => "c",
            KeyCode::D => "d",
            KeyCode::E => "e",
            KeyCode::F => "f",
            KeyCode::G => "g",
            KeyCode::H => "h",
            KeyCode::I => "i",
            KeyCode::J => "j",
            KeyCode::K => "k",
            KeyCode::L => "l",
            KeyCode::M => "m",
            KeyCode::N => "n",
            KeyCode::O => "o",
            KeyCode::P => "p",
            KeyCode::Q => "q",
            KeyCode::R => "r",
            KeyCode::S => "s",
            KeyCode::T => "t",
            KeyCode::U => "u",
            KeyCode::V => "v",
            KeyCode::W => "w",
            KeyCode::X => "x",
            KeyCode::Y => "y",
            KeyCode::Z => "z",
            KeyCode::Digit0 => "0",
            KeyCode::Digit1 => "1",
            KeyCode::Digit2 => "2",
            KeyCode::Digit3 => "3",
            KeyCode::Digit4 => "4",
            KeyCode::Digit5 => "5",
            KeyCode::Digit6 => "6",
            KeyCode::Digit7 => "7",
            KeyCode::Digit8 => "8",
            KeyCode::Digit9 => "9",
            KeyCode::Space => " ",
            KeyCode::Left => "ArrowLeft",
            KeyCode::Right => "ArrowRight",
            KeyCode::Up => "ArrowUp",
            KeyCode::Down => "ArrowDown",
            KeyCode::BracketLeft => "[",
            KeyCode::BracketRight => "]",
            KeyCode::Minus => "-",
            KeyCode::Equal => "=",
            KeyCode::Comma => ",",
            KeyCode::Period => ".",
            KeyCode::Slash => "/",
            KeyCode::Backslash => "\\",
            KeyCode::Semicolon => ";",
            KeyCode::Quote => "'",
            KeyCode::Backquote => "`",
            KeyCode::Enter => "Enter",
            KeyCode::Escape => "Escape",
            KeyCode::Tab => "Tab",
            KeyCode::Shift => "Shift",
            KeyCode::Control => "Control",
            KeyCode::Alt => "Alt",
            KeyCode::Backspace => "Backspace",
            KeyCode::Delete => "Delete",
            KeyCode::Insert => "Insert",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "PageUp",
            KeyCode::PageDown => "PageDown",
            KeyCode::F1 => "F1",
            KeyCode::F2 => "F2",
            KeyCode::F3 => "F3",
            KeyCode::F4 => "F4",
            KeyCode::F5 => "F5",
            KeyCode::F6 => "F6",
            KeyCode::F7 => "F7",
            KeyCode::F8 => "F8",
            KeyCode::F9 => "F9",
            KeyCode::F10 => "F10",
            KeyCode::F11 => "F11",
            KeyCode::F12 => "F12",
        }
    }

    /// Every `KeyCode`, in declaration order. For a settings UI that lists the
    /// rebindable keys.
    pub fn all() -> &'static [KeyCode] {
        use KeyCode::*;
        &[
            A,
            B,
            C,
            D,
            E,
            F,
            G,
            H,
            I,
            J,
            K,
            L,
            M,
            N,
            O,
            P,
            Q,
            R,
            S,
            T,
            U,
            V,
            W,
            X,
            Y,
            Z,
            Digit0,
            Digit1,
            Digit2,
            Digit3,
            Digit4,
            Digit5,
            Digit6,
            Digit7,
            Digit8,
            Digit9,
            Space,
            Left,
            Right,
            Up,
            Down,
            BracketLeft,
            BracketRight,
            Minus,
            Equal,
            Comma,
            Period,
            Slash,
            Backslash,
            Semicolon,
            Quote,
            Backquote,
            Enter,
            Escape,
            Tab,
            Shift,
            Control,
            Alt,
            Backspace,
            Delete,
            Insert,
            Home,
            End,
            PageUp,
            PageDown,
            F1,
            F2,
            F3,
            F4,
            F5,
            F6,
            F7,
            F8,
            F9,
            F10,
            F11,
            F12,
        ]
    }

    /// Parse a DOM `KeyboardEvent.key` string.
    ///
    /// Accepts both the DOM spelling (`"ArrowLeft"`, `"F1"`) and the bare
    /// character (`"a"`, `"0"`, `"["`), because which one a browser reports
    /// depends on the key and the active layout.
    pub fn from_name(name: &str) -> Option<KeyCode> {
        let canonical = match name {
            "Space" | "space" => " ",
            "Left" => "ArrowLeft",
            "Right" => "ArrowRight",
            "Up" => "ArrowUp",
            "Down" => "ArrowDown",
            "Esc" => "Escape",
            "Ctrl" => "Control",
            "ShiftLeft" | "ShiftRight" => "Shift",
            "ControlLeft" | "ControlRight" => "Control",
            "AltLeft" | "AltRight" => "Alt",
            "BracketLeft" => "[",
            "BracketRight" => "]",
            other => other,
        };
        // Looked up from `name()` rather than a second hand-written table, so
        // the two directions cannot drift apart.
        KeyCode::all()
            .iter()
            .copied()
            .find(|key| key.name() == canonical)
            .or_else(|| {
                // A platform that reports the shifted glyph ("A" for the
                // letter cluster) still means the unshifted key.
                KeyCode::all()
                    .iter()
                    .copied()
                    .find(|key| key.name().len() == 1 && key.name().eq_ignore_ascii_case(canonical))
            })
    }
}

/// Mouse buttons.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    /// A stable label, for logs and key-binding display.
    pub fn name(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }

    /// Every button, in declaration order.
    pub fn all() -> &'static [MouseButton] {
        &[MouseButton::Left, MouseButton::Right, MouseButton::Middle]
    }
}

/// A gamepad's analogue axes, as a device reports them.
///
/// Sticks are `[-1, 1]` per axis, triggers `[0, 1]`. Values arrive raw: the
/// consumer applies its own deadzone, because a game and an editor camera want
/// different ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gamepad {
    pub left: Vec2,
    pub right: Vec2,
    pub left_trigger: f32,
    pub right_trigger: f32,
}

impl Default for Gamepad {
    fn default() -> Self {
        Self {
            left: Vec2::ZERO,
            right: Vec2::ZERO,
            left_trigger: 0.0,
            right_trigger: 0.0,
        }
    }
}

impl Gamepad {
    /// Builder methods: each sets one group of axes and leaves the rest alone,
    /// so they chain to build a full pad state.
    pub fn with_left(mut self, x: f32, y: f32) -> Self {
        self.left = Vec2::new(x, y);
        self
    }

    pub fn with_right(mut self, x: f32, y: f32) -> Self {
        self.right = Vec2::new(x, y);
        self
    }

    pub fn with_triggers(mut self, left: f32, right: f32) -> Self {
        self.left_trigger = left;
        self.right_trigger = right;
        self
    }

    /// The left stick with a radial deadzone applied, so a stick resting
    /// off-axis reads as centered instead of creeping.
    pub fn left_deadzone(&self, deadzone: f32) -> Vec2 {
        Self::rescale(self.left, deadzone)
    }

    /// The right stick with a radial deadzone applied.
    pub fn right_deadzone(&self, deadzone: f32) -> Vec2 {
        Self::rescale(self.right, deadzone)
    }

    /// Rescale so the stick still reaches full range at the rim, instead of
    /// jumping from 0 straight to `deadzone` as it crosses the threshold.
    fn rescale(stick: Vec2, deadzone: f32) -> Vec2 {
        let length = stick.length();
        if length <= deadzone || length == 0.0 {
            return Vec2::ZERO;
        }
        stick * ((length - deadzone) / (length * (1.0 - deadzone)))
    }
}

/// Accumulates input state between frames. The platform feeds raw events;
/// simulation drains per-frame edge sets.
#[derive(Default)]
pub struct InputState {
    down: HashSet<KeyCode>,
    pressed: HashSet<KeyCode>,
    released: HashSet<KeyCode>,
    mouse_down: HashSet<MouseButton>,
    mouse_pressed: HashSet<MouseButton>,
    mouse_released: HashSet<MouseButton>,
    mouse_x: f32,
    mouse_y: f32,
    mouse_seen: bool,
    mouse_dx: f32,
    mouse_dy: f32,
    mouse_scroll_x: f32,
    mouse_scroll_y: f32,
    gamepad: Gamepad,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key_down(&mut self, key: KeyCode) {
        if self.down.insert(key) {
            self.pressed.insert(key);
        }
    }

    pub fn key_up(&mut self, key: KeyCode) {
        if self.down.remove(&key) {
            self.released.insert(key);
        }
    }

    pub fn is_down(&self, key: KeyCode) -> bool {
        self.down.contains(&key)
    }

    /// True when any key is held.
    pub fn any_key_down(&self) -> bool {
        !self.down.is_empty()
    }

    pub fn mouse_down(&mut self, button: MouseButton) {
        if self.mouse_down.insert(button) {
            self.mouse_pressed.insert(button);
        }
    }

    pub fn mouse_up(&mut self, button: MouseButton) {
        if self.mouse_down.remove(&button) {
            self.mouse_released.insert(button);
        }
    }

    pub fn is_mouse_down(&self, button: MouseButton) -> bool {
        self.mouse_down.contains(&button)
    }

    /// Move the cursor to an absolute position, accumulating the motion.
    ///
    /// The first move only establishes the origin: the initial `(0, 0)` is an
    /// uninitialized default rather than a real cursor position, so treating it
    /// as one would hand the opening frame a delta the player never made.
    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if self.mouse_seen {
            self.mouse_dx += x - self.mouse_x;
            self.mouse_dy += y - self.mouse_y;
        }
        self.mouse_seen = true;
        self.mouse_x = x;
        self.mouse_y = y;
    }

    pub fn mouse_x(&self) -> f32 {
        self.mouse_x
    }

    pub fn mouse_y(&self) -> f32 {
        self.mouse_y
    }

    /// Accumulate wheel / scroll-wheel motion, in the platform's own units.
    pub fn mouse_scroll(&mut self, delta_x: f32, delta_y: f32) {
        self.mouse_scroll_x += delta_x;
        self.mouse_scroll_y += delta_y;
    }

    /// Replace the analogue axes. A disconnected pad reports zeroes, which is
    /// `Gamepad::default`.
    pub fn set_gamepad(&mut self, gamepad: Gamepad) {
        self.gamepad = gamepad;
    }

    pub fn gamepad(&self) -> Gamepad {
        self.gamepad
    }

    pub fn axis_left(&mut self, x: f32, y: f32) {
        self.gamepad.left = Vec2::new(x, y);
    }

    pub fn axis_right(&mut self, x: f32, y: f32) {
        self.gamepad.right = Vec2::new(x, y);
    }

    pub fn axis_triggers(&mut self, left: f32, right: f32) {
        self.gamepad.left_trigger = left;
        self.gamepad.right_trigger = right;
    }

    /// Snapshot the current frame's edges, clearing them for the next frame.
    ///
    /// Called once per simulation tick, so a frame's events land on exactly one
    /// tick. The tick loop may run zero, one, or many times per presented
    /// frame; draining here keeps the simulation independent of display rate.
    pub fn begin_frame(&mut self) -> FrameInput {
        FrameInput {
            pressed: std::mem::take(&mut self.pressed),
            released: std::mem::take(&mut self.released),
            mouse_pressed: std::mem::take(&mut self.mouse_pressed),
            mouse_released: std::mem::take(&mut self.mouse_released),
            mouse_x: self.mouse_x,
            mouse_y: self.mouse_y,
            mouse_dx: std::mem::take(&mut self.mouse_dx),
            mouse_dy: std::mem::take(&mut self.mouse_dy),
            mouse_scroll_x: std::mem::take(&mut self.mouse_scroll_x),
            mouse_scroll_y: std::mem::take(&mut self.mouse_scroll_y),
            gamepad: self.gamepad,
        }
    }
}

/// Edges for a single simulated frame — what was pressed / released *this
/// tick*. Held state is queried via `InputState::is_down`.
#[derive(Default)]
pub struct FrameInput {
    pub pressed: HashSet<KeyCode>,
    pub released: HashSet<KeyCode>,
    pub mouse_pressed: HashSet<MouseButton>,
    pub mouse_released: HashSet<MouseButton>,
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub mouse_dx: f32,
    pub mouse_dy: f32,
    pub mouse_scroll_x: f32,
    pub mouse_scroll_y: f32,
    pub gamepad: Gamepad,
}

impl FrameInput {
    pub fn pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    pub fn released(&self, key: KeyCode) -> bool {
        self.released.contains(&key)
    }

    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse_pressed.contains(&button)
    }

    pub fn mouse_released(&self, button: MouseButton) -> bool {
        self.mouse_released.contains(&button)
    }

    /// Cursor position this frame.
    pub fn mouse_position(&self) -> Vec2 {
        Vec2::new(self.mouse_x, self.mouse_y)
    }

    /// Cursor motion accumulated since the previous frame, for drag handlers
    /// and gizmos.
    pub fn mouse_delta(&self) -> Vec2 {
        Vec2::new(self.mouse_dx, self.mouse_dy)
    }

    /// Wheel / scroll-wheel motion, in platform units.
    pub fn mouse_scroll_delta(&self) -> Vec2 {
        Vec2::new(self.mouse_scroll_x, self.mouse_scroll_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    #[test]
    fn key_edges_are_reported_once_per_frame() {
        let mut input = InputState::new();
        input.key_down(KeyCode::A);
        let frame = input.begin_frame();
        assert!(frame.pressed(KeyCode::A));
        assert!(frame.released.is_empty());

        let next = input.begin_frame();
        assert!(!next.pressed(KeyCode::A));
    }

    #[test]
    fn held_key_is_down_but_not_pressed_again() {
        let mut input = InputState::new();
        input.key_down(KeyCode::Left);
        input.begin_frame();
        assert!(input.is_down(KeyCode::Left));

        input.key_down(KeyCode::Left);
        let frame = input.begin_frame();
        assert!(!frame.pressed(KeyCode::Left));
        assert!(input.is_down(KeyCode::Left));
    }

    #[test]
    fn release_reports_edge_and_clears_held_state() {
        let mut input = InputState::new();
        input.key_down(KeyCode::Enter);
        input.begin_frame();
        input.key_up(KeyCode::Enter);
        let frame = input.begin_frame();
        assert!(frame.released(KeyCode::Enter));
        assert!(!frame.pressed(KeyCode::Enter));
        assert!(!input.is_down(KeyCode::Enter));
    }

    #[test]
    fn a_release_without_a_press_reports_nothing() {
        let mut input = InputState::new();
        input.key_up(KeyCode::Z);
        assert!(input.begin_frame().released.is_empty());
    }

    #[test]
    fn keys_are_tracked_independently() {
        let mut input = InputState::new();
        input.key_down(KeyCode::W);
        input.key_down(KeyCode::Shift);
        let frame = input.begin_frame();
        assert_eq!(frame.pressed.len(), 2);
        assert!(frame.pressed(KeyCode::W));
        assert!(frame.pressed(KeyCode::Shift));
        assert!(input.any_key_down());

        input.key_up(KeyCode::Shift);
        let frame = input.begin_frame();
        assert!(frame.released(KeyCode::Shift));
        assert!(input.is_down(KeyCode::W));
    }

    #[test]
    fn no_key_down_is_false() {
        assert!(!InputState::new().any_key_down());
    }

    #[test]
    fn key_code_from_name_maps_browser_and_desktop_names() {
        assert_eq!(KeyCode::from_name("a"), Some(KeyCode::A));
        assert_eq!(KeyCode::from_name("ArrowRight"), Some(KeyCode::Right));
        assert_eq!(KeyCode::from_name("["), Some(KeyCode::BracketLeft));
        assert_eq!(KeyCode::from_name("Space"), Some(KeyCode::Space));
    }

    #[test]
    fn key_code_from_name_handles_digits_and_punctuation() {
        assert_eq!(KeyCode::from_name("0"), Some(KeyCode::Digit0));
        assert_eq!(KeyCode::from_name("9"), Some(KeyCode::Digit9));
        assert_eq!(KeyCode::from_name(","), Some(KeyCode::Comma));
        assert_eq!(KeyCode::from_name("."), Some(KeyCode::Period));
        assert_eq!(KeyCode::from_name("-"), Some(KeyCode::Minus));
        assert_eq!(KeyCode::from_name("="), Some(KeyCode::Equal));
        assert_eq!(KeyCode::from_name("\\"), Some(KeyCode::Backslash));
        assert_eq!(KeyCode::from_name("F12"), Some(KeyCode::F12));
        assert_eq!(KeyCode::from_name("PageDown"), Some(KeyCode::PageDown));
    }

    #[test]
    fn key_code_from_name_normalizes_modifier_and_navigation_spellings() {
        assert_eq!(KeyCode::from_name("ShiftLeft"), Some(KeyCode::Shift));
        assert_eq!(KeyCode::from_name("ControlRight"), Some(KeyCode::Control));
        assert_eq!(KeyCode::from_name("AltLeft"), Some(KeyCode::Alt));
        assert_eq!(KeyCode::from_name("Esc"), Some(KeyCode::Escape));
        assert_eq!(KeyCode::from_name("Left"), Some(KeyCode::Left));
        assert_eq!(
            KeyCode::from_name("BracketRight"),
            Some(KeyCode::BracketRight)
        );
    }

    #[test]
    fn key_code_from_name_accepts_the_shifted_glyph() {
        assert_eq!(KeyCode::from_name("A"), Some(KeyCode::A));
        assert_eq!(KeyCode::from_name("Z"), Some(KeyCode::Z));
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(KeyCode::from_name("CapsLock"), None);
        assert_eq!(KeyCode::from_name(""), None);
        assert_eq!(KeyCode::from_name("ArrowUpLeft"), None);
    }

    /// Every variant must round-trip, so a new key cannot ship without a name
    /// or with a name the DOM path cannot parse back.
    #[test]
    fn every_key_round_trips_through_its_name() {
        for key in KeyCode::all() {
            assert_eq!(
                KeyCode::from_name(key.name()),
                Some(*key),
                "{key:?} does not round-trip"
            );
        }
    }

    #[test]
    fn mouse_buttons_have_distinct_names() {
        let all = MouseButton::all();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "duplicate MouseButton in all()");
                assert_ne!(a.name(), b.name(), "{a:?} and {b:?} share a name");
            }
        }
    }

    #[test]
    fn key_all_lists_every_variant_exactly_once() {
        let all = KeyCode::all();
        let mut unique = all.to_vec();
        unique.sort_by_key(|key| key.name());
        let count = unique.len();
        unique.dedup_by_key(|key| key.name());
        assert_eq!(unique.len(), count, "duplicate names in KeyCode::all");
    }

    #[test]
    fn mouse_button_edges_are_reported_once_per_frame() {
        let mut input = InputState::new();
        input.mouse_down(MouseButton::Left);
        let frame = input.begin_frame();
        assert!(frame.mouse_pressed(MouseButton::Left));
        assert!(input.is_mouse_down(MouseButton::Left));

        input.mouse_down(MouseButton::Left);
        let frame = input.begin_frame();
        assert!(!frame.mouse_pressed(MouseButton::Left));
        assert!(input.is_mouse_down(MouseButton::Left));

        input.mouse_up(MouseButton::Left);
        let frame = input.begin_frame();
        assert!(frame.mouse_released(MouseButton::Left));
        assert!(!input.is_mouse_down(MouseButton::Left));
    }

    #[test]
    fn mouse_buttons_are_tracked_independently() {
        let mut input = InputState::new();
        input.mouse_down(MouseButton::Left);
        input.mouse_down(MouseButton::Right);
        assert!(input.is_mouse_down(MouseButton::Left));
        assert!(input.is_mouse_down(MouseButton::Right));
        assert!(!input.is_mouse_down(MouseButton::Middle));

        input.mouse_up(MouseButton::Left);
        let frame = input.begin_frame();
        assert!(frame.mouse_released(MouseButton::Left));
        assert!(!frame.mouse_released(MouseButton::Right));
        assert!(input.is_mouse_down(MouseButton::Right));
    }

    #[test]
    fn a_mouse_release_without_a_press_reports_nothing() {
        let mut input = InputState::new();
        input.mouse_up(MouseButton::Middle);
        assert!(input.begin_frame().mouse_released.is_empty());
    }

    #[test]
    fn mouse_position_persists_across_frames() {
        let mut input = InputState::new();
        input.mouse_move(120.0, 45.0);
        assert_eq!(input.mouse_x(), 120.0);
        assert_eq!(input.mouse_y(), 45.0);

        let frame = input.begin_frame();
        assert_eq!(frame.mouse_position(), Vec2::new(120.0, 45.0));
        // Nothing moved since, so the delta is zero.
        assert_eq!(frame.mouse_delta(), Vec2::ZERO);

        let frame = input.begin_frame();
        assert_eq!(frame.mouse_position(), Vec2::new(120.0, 45.0));
    }

    /// The opening move must not look like a drag from the origin, or a gizmo
    /// grabbed on the first frame would jump across the screen.
    #[test]
    fn the_first_move_establishes_the_origin_without_a_delta() {
        let mut input = InputState::new();
        input.mouse_move(500.0, 400.0);
        let frame = input.begin_frame();
        assert_eq!(frame.mouse_position(), Vec2::new(500.0, 400.0));
        assert_eq!(frame.mouse_delta(), Vec2::ZERO);
    }

    #[test]
    fn motion_after_the_first_move_is_reported() {
        let mut input = InputState::new();
        input.mouse_move(10.0, 10.0);
        input.begin_frame();

        input.mouse_move(14.0, 6.0);
        let frame = input.begin_frame();
        assert_eq!(frame.mouse_delta(), Vec2::new(4.0, -4.0));
    }

    #[test]
    fn mouse_delta_sums_the_motion_within_a_frame() {
        let mut input = InputState::new();
        input.mouse_move(10.0, 10.0);
        input.begin_frame();

        input.mouse_move(12.0, 9.0);
        input.mouse_move(15.0, 9.0);
        let frame = input.begin_frame();
        assert_eq!(frame.mouse_position(), Vec2::new(15.0, 9.0));
        assert_eq!(frame.mouse_delta(), Vec2::new(5.0, -1.0));
    }

    /// A move that arrives while no tick is running must not be lost: it lands
    /// on the next tick that drains.
    #[test]
    fn a_move_waits_for_the_next_tick() {
        let mut input = InputState::new();
        input.mouse_move(4.0, 4.0);
        input.mouse_move(7.0, 4.0);
        let frame = input.begin_frame();
        assert_eq!(frame.mouse_delta(), Vec2::new(3.0, 0.0));
    }

    #[test]
    fn scroll_accumulates_and_then_clears() {
        let mut input = InputState::new();
        input.mouse_scroll(0.0, 1.0);
        input.mouse_scroll(0.0, 2.0);
        let frame = input.begin_frame();
        assert_eq!(frame.mouse_scroll_delta(), Vec2::new(0.0, 3.0));

        let frame = input.begin_frame();
        assert_eq!(frame.mouse_scroll_delta(), Vec2::ZERO);
    }

    #[test]
    fn a_default_gamepad_is_centered() {
        let gamepad = Gamepad::default();
        assert_eq!(gamepad.left, Vec2::ZERO);
        assert_eq!(gamepad.right, Vec2::ZERO);
        assert_eq!(gamepad.left_trigger, 0.0);
        assert_eq!(gamepad.right_trigger, 0.0);
    }

    #[test]
    fn gamepad_axes_reach_the_frame_unchanged() {
        let mut input = InputState::new();
        input.set_gamepad(
            Gamepad::default()
                .with_left(0.5, -0.25)
                .with_right(-1.0, 0.75)
                .with_triggers(1.0, 0.5),
        );
        let frame = input.begin_frame();
        assert_eq!(frame.gamepad.left, Vec2::new(0.5, -0.25));
        assert_eq!(frame.gamepad.right, Vec2::new(-1.0, 0.75));
        assert_eq!(frame.gamepad.left_trigger, 1.0);
        assert_eq!(frame.gamepad.right_trigger, 0.5);
    }

    /// Analogue state is level-triggered, not an edge, so it persists across
    /// frames instead of being drained.
    #[test]
    fn gamepad_axes_are_level_triggered() {
        let mut input = InputState::new();
        input.axis_left(0.0, -1.0);
        input.begin_frame();
        let frame = input.begin_frame();
        assert_eq!(frame.gamepad.left, Vec2::new(0.0, -1.0));
        assert_eq!(input.gamepad().left, Vec2::new(0.0, -1.0));
    }

    #[test]
    fn axis_setters_and_set_gamepad_agree() {
        let mut input = InputState::new();
        input.axis_left(1.0, 2.0);
        input.axis_right(3.0, 4.0);
        input.axis_triggers(5.0, 6.0);
        let by_setters = input.gamepad();

        let mut other = InputState::new();
        other.set_gamepad(Gamepad {
            left: Vec2::new(1.0, 2.0),
            right: Vec2::new(3.0, 4.0),
            left_trigger: 5.0,
            right_trigger: 6.0,
        });
        assert_eq!(by_setters, other.gamepad());
    }

    #[test]
    fn disconnecting_a_pad_zeroes_the_axes() {
        let mut input = InputState::new();
        input.set_gamepad(
            Gamepad::default()
                .with_left(1.0, 1.0)
                .with_triggers(1.0, 1.0),
        );
        input.set_gamepad(Gamepad::default());
        assert_eq!(input.gamepad(), Gamepad::default());
    }

    #[test]
    fn a_stick_inside_the_deadzone_reads_as_centered() {
        let pad = Gamepad::default().with_left(0.05, 0.0);
        assert_eq!(pad.left_deadzone(0.2), Vec2::ZERO);
        assert_eq!(Gamepad::default().left_deadzone(0.2), Vec2::ZERO);
    }

    #[test]
    fn a_stick_at_the_rim_reaches_full_range_after_the_deadzone() {
        let pad = Gamepad::default().with_left(1.0, 0.0);
        let out = pad.left_deadzone(0.2);
        assert!((out.x - 1.0).abs() < EPS, "rim input was {out:?}");
    }

    #[test]
    fn the_deadzone_rescales_instead_of_jumping() {
        // Just past the threshold should report a small value, not 0.2.
        let pad = Gamepad::default().with_left(0.25, 0.0);
        let out = pad.left_deadzone(0.2);
        assert!(out.x > 0.0 && out.x < 0.25, "got {out:?}");
    }

    #[test]
    fn the_deadzone_preserves_direction() {
        let pad = Gamepad::default().with_left(0.6, 0.8);
        let out = pad.left_deadzone(0.2);
        let before = Vec2::new(0.6, 0.8).normalize();
        let after = out.normalize();
        assert!((before.x - after.x).abs() < EPS);
        assert!((before.y - after.y).abs() < EPS);
    }

    #[test]
    fn deadzones_are_independent_per_stick() {
        let pad = Gamepad::default().with_left(1.0, 0.0).with_right(0.05, 0.0);
        assert!((pad.left_deadzone(0.2).x - 1.0).abs() < EPS);
        assert_eq!(pad.right_deadzone(0.2), Vec2::ZERO);
    }

    #[test]
    fn keys_and_pointer_edges_are_reported_together() {
        let mut input = InputState::new();
        input.key_down(KeyCode::Space);
        input.mouse_down(MouseButton::Middle);
        input.mouse_move(3.0, 4.0);
        input.mouse_scroll(0.0, 5.0);
        let frame = input.begin_frame();
        assert!(frame.pressed(KeyCode::Space));
        assert!(frame.mouse_pressed(MouseButton::Middle));
        assert_eq!(frame.mouse_position(), Vec2::new(3.0, 4.0));
        assert_eq!(frame.mouse_scroll_delta(), Vec2::new(0.0, 5.0));
    }
}
