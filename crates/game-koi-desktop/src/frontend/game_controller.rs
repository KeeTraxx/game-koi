//! Gamepad input, via gilrs.
//!
//! Unlike the keyboard, a gamepad is not a source of winit events — gilrs opens the
//! devices itself and keeps its own queue, so we drain that queue once per frame in
//! [`super::App::about_to_wait`] rather than reacting to callbacks. One frame (16.7 ms)
//! of latency is the same granularity the emulator already runs at, so nothing is lost
//! by not polling more often.
//!
//! # Analog sticks are not a d-pad
//!
//! The DMG's d-pad is four switches: a direction is held or it isn't. A stick reports a
//! continuous position, so we threshold it and remember which direction is currently
//! "held" per axis, emitting a press only when that crossing happens rather than on
//! every axis report. Without that memory a stick resting at 0.7 would re-press Right
//! hundreds of times a second.

use gilrs::{Axis, Button as PadButton, EventType, Gilrs};

use super::Action;
use game_koi_core::joypad::Button;

/// How far a stick must be pushed before it counts as a direction press.
///
/// Half deflection: high enough that a drifting or off-centre stick does not hold a
/// direction on its own, low enough that a diagonal registers on both axes.
const STICK_THRESHOLD: f32 = 0.5;

/// A connected gamepad, or nothing if none could be opened.
pub struct GameController {
    /// `None` when gilrs could not start — no udev, no permissions, unsupported
    /// platform. A missing gamepad is not an error worth refusing to run over; the
    /// keyboard still works, so we warn once and carry on.
    gilrs: Option<Gilrs>,
    /// The direction each analog axis is currently holding, if any. Two axes for the
    /// stick and two for the d-pad, because on Linux a hat switch is often reported as
    /// a pair of axes rather than as four buttons.
    stick_x: Option<Button>,
    stick_y: Option<Button>,
    hat_x: Option<Button>,
    hat_y: Option<Button>,
}

impl GameController {
    /// Opens whatever gamepads are attached. Never fails; see the `gilrs` field.
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(gilrs) => Some(gilrs),
            Err(err) => {
                eprintln!("warning: gamepad support unavailable ({err}); keyboard only");
                None
            }
        };

        Self {
            gilrs,
            stick_x: None,
            stick_y: None,
            hat_x: None,
            hat_y: None,
        }
    }

    /// Drains everything that has happened since the last call.
    ///
    /// Returns a `Vec` rather than taking a callback so the caller can apply the
    /// actions to the bus without holding a borrow of `self` across the loop.
    ///
    /// Events from *every* connected pad are accepted, since the Game Boy has only one
    /// player and asking which controller is "player 1" would be ceremony for nothing.
    pub fn poll(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        let Some(gilrs) = self.gilrs.as_mut() else {
            return actions;
        };

        while let Some(event) = gilrs.next_event() {
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(button) = map_pad_button(button) {
                        actions.push(Action::Press(button));
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(button) = map_pad_button(button) {
                        actions.push(Action::Release(button));
                    }
                }
                EventType::AxisChanged(axis, value, _) => {
                    // Which stored direction this axis owns, and what the two ends of
                    // it mean. gilrs reports Y positive as up, matching the stick.
                    let (held, direction) = match axis {
                        Axis::LeftStickX => (
                            &mut self.stick_x,
                            axis_direction(value, Button::Left, Button::Right),
                        ),
                        Axis::LeftStickY => (
                            &mut self.stick_y,
                            axis_direction(value, Button::Down, Button::Up),
                        ),
                        Axis::DPadX => (
                            &mut self.hat_x,
                            axis_direction(value, Button::Left, Button::Right),
                        ),
                        Axis::DPadY => (
                            &mut self.hat_y,
                            axis_direction(value, Button::Down, Button::Up),
                        ),
                        _ => continue,
                    };
                    push_axis_change(held, direction, &mut actions);
                }
                _ => {}
            }
        }

        actions
    }
}

/// Maps a physical gamepad button to a Game Boy one.
///
/// A and B sit on a diagonal on the DMG — B lower-left, A upper-right — so mapping
/// them to the South and East face buttons reproduces that under the thumb. It also
/// happens to match the printed labels on a Nintendo-layout pad, where East *is* A.
fn map_pad_button(button: PadButton) -> Option<Button> {
    Some(match button {
        PadButton::DPadUp => Button::Up,
        PadButton::DPadDown => Button::Down,
        PadButton::DPadLeft => Button::Left,
        PadButton::DPadRight => Button::Right,
        PadButton::East => Button::A,
        PadButton::South => Button::B,
        PadButton::Start => Button::Start,
        PadButton::Select => Button::Select,
        _ => return None,
    })
}

/// Which direction an axis position is currently holding, if any.
fn axis_direction(value: f32, negative: Button, positive: Button) -> Option<Button> {
    if value <= -STICK_THRESHOLD {
        Some(negative)
    } else if value >= STICK_THRESHOLD {
        Some(positive)
    } else {
        None
    }
}

/// Records an axis moving to a new direction, emitting the press/release edges.
///
/// Nothing is emitted while the direction is unchanged, which is what makes a held
/// stick behave like a held switch.
fn push_axis_change(held: &mut Option<Button>, next: Option<Button>, actions: &mut Vec<Action>) {
    if *held == next {
        return;
    }
    // Release first: moving from Left straight to Right must not leave Left stuck.
    if let Some(previous) = held.take() {
        actions.push(Action::Release(previous));
    }
    if let Some(button) = next {
        actions.push(Action::Press(button));
    }
    *held = next;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_mapping_covers_all_eight_buttons() {
        let buttons = [
            PadButton::DPadUp,
            PadButton::DPadDown,
            PadButton::DPadLeft,
            PadButton::DPadRight,
            PadButton::East,
            PadButton::South,
            PadButton::Start,
            PadButton::Select,
        ];
        let mapped: Vec<Button> = buttons.into_iter().filter_map(map_pad_button).collect();
        assert_eq!(mapped.len(), 8, "every button has a pad binding");

        // Buttons the Game Boy does not have stay unmapped.
        assert!(map_pad_button(PadButton::LeftTrigger).is_none());
        assert!(map_pad_button(PadButton::Mode).is_none());
    }

    #[test]
    fn a_stick_at_rest_holds_nothing() {
        assert_eq!(axis_direction(0.0, Button::Left, Button::Right), None);
        // Drift below the threshold must not hold a direction.
        assert_eq!(axis_direction(0.3, Button::Left, Button::Right), None);
        assert_eq!(axis_direction(-0.3, Button::Left, Button::Right), None);
    }

    #[test]
    fn a_pushed_stick_holds_one_direction() {
        assert_eq!(
            axis_direction(-1.0, Button::Left, Button::Right),
            Some(Button::Left)
        );
        assert_eq!(
            axis_direction(1.0, Button::Left, Button::Right),
            Some(Button::Right)
        );
        // A hat reported as an axis uses the same -1/0/1 range.
        assert_eq!(
            axis_direction(1.0, Button::Down, Button::Up),
            Some(Button::Up)
        );
    }

    #[test]
    fn holding_a_direction_presses_once() {
        let mut held = None;
        let mut actions = Vec::new();

        push_axis_change(&mut held, Some(Button::Right), &mut actions);
        assert_eq!(actions, vec![Action::Press(Button::Right)]);

        // The stick is still deflected, but nothing has changed: no repeat press.
        actions.clear();
        push_axis_change(&mut held, Some(Button::Right), &mut actions);
        assert!(actions.is_empty());

        // Back to centre releases it.
        push_axis_change(&mut held, None, &mut actions);
        assert_eq!(actions, vec![Action::Release(Button::Right)]);
        assert_eq!(held, None);
    }

    #[test]
    fn flicking_across_centre_releases_before_pressing() {
        // If the release did not come first, the joypad would see Left and Right held
        // at once — a state the physical d-pad cannot produce, and one some games
        // handle badly.
        let mut held = Some(Button::Left);
        let mut actions = Vec::new();
        push_axis_change(&mut held, Some(Button::Right), &mut actions);
        assert_eq!(
            actions,
            vec![Action::Release(Button::Left), Action::Press(Button::Right)]
        );
    }
}
