//! Keyboard input: host keys in, [`Action`]s out.
//!
//! This module knows nothing about the window or the emulator — it is a pure
//! translation from a winit key event to the frontend's own [`Action`] vocabulary, so
//! it can be unit tested without a display. The gamepad path in
//! [`super::game_controller`] produces the same `Action`s, which is what keeps the two
//! input sources from each needing their own copy of the "press this button" logic.

use winit::event::ElementState;
use winit::keyboard::KeyCode;

use super::Action;
use crate::joypad::Button;

/// Maps a host key to a Game Boy button.
///
/// The d-pad on arrows and A/B on Z/X is the layout every emulator uses, so muscle
/// memory carries over.
pub fn map_key(key: KeyCode) -> Option<Button> {
    Some(match key {
        KeyCode::ArrowUp => Button::Up,
        KeyCode::ArrowDown => Button::Down,
        KeyCode::ArrowLeft => Button::Left,
        KeyCode::ArrowRight => Button::Right,
        KeyCode::KeyZ => Button::A,
        KeyCode::KeyX => Button::B,
        KeyCode::Enter => Button::Start,
        KeyCode::ShiftRight => Button::Select,
        _ => return None,
    })
}

/// Turns a key event into the action it should trigger, if any.
///
/// Escape, P, F1 and F2 are host-side controls: the emulated machine never sees them,
/// because the DMG had no such buttons. Everything else falls through to [`map_key`].
pub fn action_for(key: KeyCode, state: ElementState) -> Option<Action> {
    if state == ElementState::Pressed {
        match key {
            KeyCode::Escape => return Some(Action::Quit),
            KeyCode::KeyP => return Some(Action::TogglePause),
            KeyCode::F1 => return Some(Action::ToggleOverlay),
            KeyCode::F2 => return Some(Action::ToggleVsync),
            _ => {}
        }
    }

    let button = map_key(key)?;
    Some(match state {
        ElementState::Pressed => Action::Press(button),
        ElementState::Released => Action::Release(button),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_covers_all_eight_buttons() {
        let keys = [
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::KeyZ,
            KeyCode::KeyX,
            KeyCode::Enter,
            KeyCode::ShiftRight,
        ];
        let mapped: Vec<Button> = keys.into_iter().filter_map(map_key).collect();
        assert_eq!(mapped.len(), 8, "every button has a key");

        // Unbound keys map to nothing rather than a default button.
        assert!(map_key(KeyCode::KeyQ).is_none());
    }

    #[test]
    fn host_controls_are_not_game_boy_buttons() {
        // Escape and P must not also reach the joypad, or pausing would press a button.
        assert!(map_key(KeyCode::Escape).is_none());
        assert!(map_key(KeyCode::KeyP).is_none());

        assert_eq!(
            action_for(KeyCode::Escape, ElementState::Pressed),
            Some(Action::Quit)
        );
        assert_eq!(
            action_for(KeyCode::KeyP, ElementState::Pressed),
            Some(Action::TogglePause)
        );

        // Host controls fire on the press only; the release is not a second toggle.
        assert_eq!(action_for(KeyCode::KeyP, ElementState::Released), None);
    }

    #[test]
    fn buttons_report_both_edges() {
        assert_eq!(
            action_for(KeyCode::KeyZ, ElementState::Pressed),
            Some(Action::Press(Button::A))
        );
        assert_eq!(
            action_for(KeyCode::KeyZ, ElementState::Released),
            Some(Action::Release(Button::A))
        );
    }
}
