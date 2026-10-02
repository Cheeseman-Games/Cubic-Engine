//! Wasm (DOM) input adapter.
//!
//! Attaches listeners to the page window and folds them into a
//! `cubic_core::input::InputState`, mirroring [`crate::platform::native`] so
//! gameplay is identical on both hosts.
//!
//! The listeners are held as `Closure`s for as long as the adapter lives, and
//! detached on drop. The alternative — `Closure::forget` — leaks the closure and
//! its `InputState` for the life of the page.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use cubic_core::input::{FrameInput, InputState, KeyCode, MouseButton};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Event, KeyboardEvent, MouseEvent, WheelEvent, Window};

use crate::platform::PIXELS_PER_SCROLL_LINE;

/// Shared between the DOM callbacks and the game's frame loop.
pub type SharedInput = Rc<RefCell<InputState>>;

/// A listener and the event name it is registered under, so [`WebInput`] can
/// detach exactly what it attached.
type Listener = (&'static str, Closure<dyn FnMut(Event)>);

/// Owns the window listeners and the state they write into.
///
/// Dropping it removes the listeners, so a game can tear input down and back up
/// (a level reload, or an editor leaving play mode) without stacking handlers.
pub struct WebInput {
    state: SharedInput,
    target: Option<Window>,
    listeners: Vec<Listener>,
}

impl WebInput {
    /// Register listeners on `window`. Keyboard and mouse events bubble to the
    /// window, so the page's own canvas does not need to forward them.
    pub fn new(window: &Window) -> Result<Self, JsValue> {
        let state: SharedInput = Rc::new(RefCell::new(InputState::new()));
        let mut input = Self {
            state,
            target: Some(window.clone()),
            listeners: Vec::new(),
        };

        input.listen(window, "keydown", |state, event| {
            let Ok(event) = event.dyn_into::<KeyboardEvent>() else {
                return;
            };
            // Stops the page scrolling under the canvas on arrows / space.
            event.prevent_default();
            if let Some(key) = KeyCode::from_name(&event.key()) {
                state.borrow_mut().key_down(key);
            }
        })?;

        input.listen(window, "keyup", |state, event| {
            let Ok(event) = event.dyn_into::<KeyboardEvent>() else {
                return;
            };
            if let Some(key) = KeyCode::from_name(&event.key()) {
                state.borrow_mut().key_up(key);
            }
        })?;

        input.listen(window, "mousemove", |state, event| {
            let Ok(event) = event.dyn_into::<MouseEvent>() else {
                return;
            };
            state
                .borrow_mut()
                .mouse_move(event.client_x() as f32, event.client_y() as f32);
        })?;

        input.listen(window, "mousedown", |state, event| {
            let Ok(event) = event.dyn_into::<MouseEvent>() else {
                return;
            };
            if let Some(button) = mouse_button_of(event.button()) {
                state.borrow_mut().mouse_down(button);
            }
        })?;

        input.listen(window, "mouseup", |state, event| {
            let Ok(event) = event.dyn_into::<MouseEvent>() else {
                return;
            };
            if let Some(button) = mouse_button_of(event.button()) {
                state.borrow_mut().mouse_up(button);
            }
        })?;

        // The wheel listener is left non-passive so a host that honours
        // `prevent_default` can suppress page scroll. The default passive
        // registration only warns when it is actually called, so it is not
        // called here.
        input.listen(window, "wheel", |state, event| {
            let Ok(event) = event.dyn_into::<WheelEvent>() else {
                return;
            };
            let (mut x, mut y) = (event.delta_x(), event.delta_y());
            if event.delta_mode() != WheelEvent::DOM_DELTA_LINE {
                // A trackpad reports pixels; scale to lines to match the
                // native adapter. Both sides of the divide are f64.
                x /= PIXELS_PER_SCROLL_LINE as f64;
                y /= PIXELS_PER_SCROLL_LINE as f64;
            }
            state.borrow_mut().mouse_scroll(x as f32, y as f32);
        })?;

        Ok(input)
    }

    fn listen<F>(&mut self, window: &Window, name: &'static str, handler: F) -> Result<(), JsValue>
    where
        F: FnMut(&SharedInput, Event) + 'static,
    {
        let state = Rc::clone(&self.state);
        let mut handler = handler;
        let closure = Closure::wrap(Box::new(move |event: Event| {
            handler(&state, event);
        }) as Box<dyn FnMut(Event)>);
        window.add_event_listener_with_callback(name, closure.as_ref().unchecked_ref())?;
        self.listeners.push((name, closure));
        Ok(())
    }

    /// The state the listeners write into, for the frame loop to borrow.
    pub fn state(&self) -> &SharedInput {
        &self.state
    }

    /// Borrow the accumulated input.
    pub fn input(&self) -> Ref<'_, InputState> {
        self.state.borrow()
    }

    /// Borrow the accumulated input mutably.
    pub fn input_mut(&self) -> RefMut<'_, InputState> {
        self.state.borrow_mut()
    }

    /// Drain this frame's edges. Call once per simulation tick.
    pub fn begin_frame(&self) -> FrameInput {
        self.state.borrow_mut().begin_frame()
    }
}

impl Drop for WebInput {
    fn drop(&mut self) {
        let Some(window) = self.target.take() else {
            return;
        };
        for (name, closure) in self.listeners.drain(..) {
            // The page may already be tearing down; a failed detach is not
            // worth panicking over during a drop.
            let _ =
                window.remove_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
        }
    }
}

/// DOM `MouseEvent.button`: 0 primary, 1 middle, 2 secondary.
fn mouse_button_of(button: i16) -> Option<MouseButton> {
    match button {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dom_button_numbering_maps_to_the_engine_buttons() {
        assert_eq!(mouse_button_of(0), Some(MouseButton::Left));
        assert_eq!(mouse_button_of(1), Some(MouseButton::Middle));
        assert_eq!(mouse_button_of(2), Some(MouseButton::Right));
        // 3/4 are back and forward; there is no engine equivalent yet.
        assert_eq!(mouse_button_of(3), None);
        assert_eq!(mouse_button_of(4), None);
    }
}
