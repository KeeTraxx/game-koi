//! The joypad: register P1 at 0xFF00.
//!
//! Eight buttons are read through four pins, using a matrix. The program writes
//! P15/P14 (bits 5 and 4) to select *which* group of four it wants, then reads
//! P13-P10 (bits 3-0) to get their state. Per register 10.1 of the reference the
//! select bits are write-only and the input bits are read-only.
//!
//! # Everything is inverted
//!
//! A pressed button reads **0**, not 1, and a group is selected by writing **0** to
//! its select bit. This is not a quirk of the register layout — the buttons are
//! wired to pull the line low, which is how essentially all matrix keypads work. It
//! trips people up constantly, so all the inversion is kept in one place here and
//! the public API speaks in plain "pressed = true".

use crate::interrupts::{Interrupt, InterruptState};

/// The eight buttons, grouped as the hardware reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    // Selected by P14: the d-pad.
    Right,
    Left,
    Up,
    Down,
    // Selected by P15: the action buttons.
    A,
    B,
    Select,
    Start,
}

impl Button {
    /// Which of the four input lines this button pulls low.
    fn line(self) -> u8 {
        match self {
            Button::Right | Button::A => 0,
            Button::Left | Button::B => 1,
            Button::Up | Button::Select => 2,
            Button::Down | Button::Start => 3,
        }
    }

    fn is_direction(self) -> bool {
        matches!(
            self,
            Button::Right | Button::Left | Button::Up | Button::Down
        )
    }
}

pub struct Joypad {
    /// Currently held buttons, as a bitmask. Directions in the low nibble, actions
    /// in the high nibble. Stored non-inverted: a set bit means held.
    pressed: u8,
    /// The last value written to the select bits (P15/P14), kept as written.
    select: u8,
}

impl Joypad {
    pub fn new() -> Self {
        Joypad {
            pressed: 0,
            // Nothing selected: both select bits high.
            select: 0x30,
        }
    }

    fn mask(button: Button) -> u8 {
        let shift = if button.is_direction() { 0 } else { 4 };
        1 << (button.line() + shift)
    }

    /// Records a button going down, raising the joypad interrupt.
    ///
    /// The interrupt fires on a press (a line going high-to-low), not a release.
    /// Its only real use is waking the CPU from STOP, since games poll P1 rather
    /// than relying on it.
    pub fn press(&mut self, button: Button, interrupts: &mut InterruptState) {
        let mask = Self::mask(button);
        let was_pressed = self.pressed & mask != 0;
        self.pressed |= mask;
        if !was_pressed {
            interrupts.request(Interrupt::Joypad);
        }
    }

    pub fn release(&mut self, button: Button) {
        self.pressed &= !Self::mask(button);
    }

    /// Reads P1.
    ///
    /// Returns the selected group's state in the low nibble, inverted so a held
    /// button reads 0. Bits 6-7 are unimplemented and read as 1.
    pub fn read(&self) -> u8 {
        // A group is selected when its bit was written *low*.
        let directions_selected = self.select & 0x10 == 0;
        let actions_selected = self.select & 0x20 == 0;

        let mut lines = 0x0F;
        if directions_selected {
            lines &= !(self.pressed & 0x0F);
        }
        if actions_selected {
            lines &= !((self.pressed >> 4) & 0x0F);
        }

        0xC0 | self.select | lines
    }

    /// Writes P1. Only the two select bits are writable.
    pub fn write(&mut self, value: u8) {
        self.select = value & 0x30;
    }
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interrupts() -> InterruptState {
        InterruptState {
            enabled: 0xFF,
            ..Default::default()
        }
    }

    #[test]
    fn nothing_pressed_reads_all_ones() {
        let mut pad = Joypad::new();
        pad.write(0x20); // select directions: P14 low, P15 high
        assert_eq!(pad.read() & 0x0F, 0x0F, "no button pulls a line low");
    }

    #[test]
    fn a_pressed_button_reads_zero() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        pad.press(Button::Right, &mut ints);

        // Select the d-pad by writing its select bit (P14, bit 4) *low*.
        pad.write(0x20);
        assert_eq!(pad.read() & 0x01, 0, "Right pulls line 0 low");
        assert_eq!(pad.read() & 0x0E, 0x0E, "the other lines stay high");
    }

    #[test]
    fn groups_are_read_separately() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        // Right and A share line 0 but are in different groups.
        pad.press(Button::Right, &mut ints);

        pad.write(0x20); // directions selected (P14 low)
        assert_eq!(pad.read() & 0x01, 0, "Right is visible");

        pad.write(0x10); // actions selected (P15 low)
        assert_eq!(pad.read() & 0x01, 0x01, "A is not pressed");

        pad.press(Button::A, &mut ints);
        assert_eq!(pad.read() & 0x01, 0, "now A is visible");
    }

    #[test]
    fn selecting_neither_group_reads_all_high() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        pad.press(Button::Start, &mut ints);
        pad.write(0x30); // both select bits high: nothing selected
        assert_eq!(pad.read() & 0x0F, 0x0F);
    }

    #[test]
    fn selecting_both_groups_merges_them() {
        // Writing both bits low selects both groups at once; the lines are ANDed,
        // so a button in either group pulls its line low.
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        pad.press(Button::Down, &mut ints); // line 3, directions
        pad.press(Button::A, &mut ints); // line 0, actions

        pad.write(0x00);
        assert_eq!(pad.read() & 0x08, 0, "Down still reads");
        assert_eq!(pad.read() & 0x01, 0, "A also reads");
    }

    #[test]
    fn releasing_restores_the_line() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        pad.write(0x20);
        pad.press(Button::Up, &mut ints);
        assert_eq!(pad.read() & 0x04, 0);
        pad.release(Button::Up);
        assert_eq!(pad.read() & 0x04, 0x04);
    }

    #[test]
    fn pressing_raises_an_interrupt_once() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();

        pad.press(Button::B, &mut ints);
        assert_eq!(ints.pending(), Some(Interrupt::Joypad));

        // Holding it does not re-raise.
        ints.acknowledge(Interrupt::Joypad);
        pad.press(Button::B, &mut ints);
        assert_eq!(ints.pending(), None, "still held, no new edge");

        // Releasing and pressing again does.
        pad.release(Button::B);
        pad.press(Button::B, &mut ints);
        assert_eq!(ints.pending(), Some(Interrupt::Joypad));
    }

    #[test]
    fn select_bits_are_readable_but_inputs_are_not_writable() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        pad.press(Button::Start, &mut ints);

        // Writing to the input bits must not clear the pressed state.
        // 0x1F selects the action group (P15 low) and sets every input bit.
        pad.write(0x1F);
        assert_eq!(pad.read() & 0x30, 0x10, "only the select bits stored");
        assert_eq!(pad.read() & 0x08, 0, "Start is still held");
        // The unimplemented top bits always read high.
        assert_eq!(pad.read() & 0xC0, 0xC0);
    }

    /// A CPU program reading the joypad the way a real game does: select a group,
    /// then read the input lines back.
    #[test]
    fn a_program_can_read_a_held_button() {
        let mut pad = Joypad::new();
        let mut ints = interrupts();
        pad.press(Button::Start, &mut ints);

        // Games write 0x20 to select actions (P15 low), then read P1. Hardware
        // needs a couple of cycles for the lines to settle; we are instantaneous.
        pad.write(0x10);
        let p1 = pad.read();

        // Start is line 3 of the action group.
        assert_eq!(p1 & 0x08, 0, "Start reads low while held");
        assert_eq!(p1 & 0x07, 0x07, "the other action buttons read high");
    }
}
