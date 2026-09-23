use std::collections::HashSet;

/// Physical keys the game understands. The platform layer maps raw
/// events into these; gameplay code never touches real keyboards.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyCode {
    A,
    D,
    X,
    C,
    Left,
    Right,
    Up,
    Down,
    BracketLeft,
    BracketRight,
    Enter,
    Escape,
}

impl KeyCode {
    /// Convert a browser `KeyboardEvent.key()` string into a `KeyCode`.
    pub fn from_name(name: &str) -> Option<KeyCode> {
        match name {
            "a" | "A" => Some(KeyCode::A),
            "d" | "D" => Some(KeyCode::D),
            "x" | "X" => Some(KeyCode::X),
            "c" | "C" => Some(KeyCode::C),
            "ArrowLeft" => Some(KeyCode::Left),
            "ArrowRight" => Some(KeyCode::Right),
            "ArrowUp" => Some(KeyCode::Up),
            "ArrowDown" => Some(KeyCode::Down),
            "[" => Some(KeyCode::BracketLeft),
            "]" => Some(KeyCode::BracketRight),
            "Enter" => Some(KeyCode::Enter),
            "Escape" => Some(KeyCode::Escape),
            _ => None,
        }
    }
}

/// Accumulates key state between frames. The platform feeds raw
/// key-down / key-up events; simulation drains per-frame edge sets.
#[derive(Default)]
pub struct InputState {
    down: HashSet<KeyCode>,
    pressed: HashSet<KeyCode>,
    released: HashSet<KeyCode>,
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

    /// Snapshot the current frame's edges, clearing them for the next frame.
    pub fn begin_frame(&mut self) -> FrameInput {
        FrameInput {
            pressed: std::mem::take(&mut self.pressed),
            released: std::mem::take(&mut self.released),
        }
    }
}

/// Edges for a single simulated frame — what was pressed / released *this
/// tick*. Held state is queried via `InputState::is_down`.
#[derive(Default)]
pub struct FrameInput {
    pub pressed: HashSet<KeyCode>,
    pub released: HashSet<KeyCode>,
}

impl FrameInput {
    pub fn pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }
}