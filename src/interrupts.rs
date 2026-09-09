//! Interrupt flags and the five interrupt sources.
//!
//! Two registers control interrupts, and they have the same bit layout:
//!
//! - `IF` (0xFF0F) — which interrupts are *requested*. Hardware sets these bits;
//!   the handler clears them.
//! - `IE` (0xFFFF) — which interrupts are *enabled*. The program sets these.
//!
//! An interrupt fires when the same bit is set in both, and the CPU's `IME` flag is
//! on. The bit order is a priority order: lower bits win when several are pending at
//! once.

/// The five interrupt sources, in hardware priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// The PPU finished drawing a frame. The main synchronisation point for games.
    VBlank,
    /// A PPU status condition the program asked to be told about.
    Stat,
    /// The timer overflowed.
    Timer,
    /// A serial transfer completed.
    Serial,
    /// A button was pressed.
    Joypad,
}

impl Interrupt {
    /// Every source, in the order the CPU checks them.
    pub const ALL: [Interrupt; 5] = [
        Interrupt::VBlank,
        Interrupt::Stat,
        Interrupt::Timer,
        Interrupt::Serial,
        Interrupt::Joypad,
    ];

    pub fn bit(self) -> u8 {
        match self {
            Interrupt::VBlank => 0,
            Interrupt::Stat => 1,
            Interrupt::Timer => 2,
            Interrupt::Serial => 3,
            Interrupt::Joypad => 4,
        }
    }

    pub fn mask(self) -> u8 {
        1 << self.bit()
    }

    /// The fixed address the CPU jumps to when this interrupt is dispatched.
    ///
    /// They sit 8 bytes apart starting at 0x0040 — the same spacing as the RST
    /// vectors, and for the same reason: 8 bytes is enough for a jump to the real
    /// handler.
    pub fn handler(self) -> u16 {
        0x0040 + self.bit() as u16 * 8
    }
}

/// The `IF`/`IE` register pair.
#[derive(Debug, Clone, Copy, Default)]
pub struct InterruptState {
    /// `IF` at 0xFF0F. Only the low 5 bits are wired up.
    pub requested: u8,
    /// `IE` at 0xFFFF. All 8 bits are readable and writable even though only 5 do
    /// anything — the reference calls the top three `IE_UNUSED`.
    pub enabled: u8,
}

impl InterruptState {
    /// Flags a source as requesting service. Called by the timer, PPU, and so on.
    pub fn request(&mut self, interrupt: Interrupt) {
        self.requested |= interrupt.mask();
    }

    pub fn acknowledge(&mut self, interrupt: Interrupt) {
        self.requested &= !interrupt.mask();
    }

    /// The highest-priority interrupt that is both requested and enabled.
    ///
    /// Deliberately independent of `IME`: a halted CPU wakes on a pending interrupt
    /// even when `IME` is off, so the two questions ("is one pending?" and "may we
    /// dispatch it?") have to stay separate.
    pub fn pending(&self) -> Option<Interrupt> {
        let active = self.requested & self.enabled & 0x1F;
        Interrupt::ALL
            .into_iter()
            .find(|interrupt| active & interrupt.mask() != 0)
    }

    /// True if any enabled interrupt is requested.
    #[cfg(test)]
    pub fn any_pending(&self) -> bool {
        self.requested & self.enabled & 0x1F != 0
    }

    /// Reads `IF`. The unused top three bits read back as 1 on real hardware.
    pub fn read_if(&self) -> u8 {
        self.requested | 0xE0
    }

    pub fn write_if(&mut self, value: u8) {
        self.requested = value & 0x1F;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handlers_are_eight_bytes_apart() {
        assert_eq!(Interrupt::VBlank.handler(), 0x0040);
        assert_eq!(Interrupt::Stat.handler(), 0x0048);
        assert_eq!(Interrupt::Timer.handler(), 0x0050);
        assert_eq!(Interrupt::Serial.handler(), 0x0058);
        assert_eq!(Interrupt::Joypad.handler(), 0x0060);
    }

    #[test]
    fn lower_bits_take_priority() {
        let mut state = InterruptState {
            enabled: 0x1F,
            ..Default::default()
        };
        state.request(Interrupt::Joypad);
        state.request(Interrupt::Timer);
        state.request(Interrupt::VBlank);
        assert_eq!(state.pending(), Some(Interrupt::VBlank));

        state.acknowledge(Interrupt::VBlank);
        assert_eq!(state.pending(), Some(Interrupt::Timer));
    }

    #[test]
    fn both_registers_must_agree() {
        let mut state = InterruptState::default();
        state.request(Interrupt::Timer);
        assert_eq!(state.pending(), None, "requested but not enabled");

        state.enabled = Interrupt::Timer.mask();
        assert_eq!(state.pending(), Some(Interrupt::Timer));

        // Enabled but not requested is equally inert.
        state.acknowledge(Interrupt::Timer);
        assert_eq!(state.pending(), None);
    }

    #[test]
    fn unused_if_bits_read_as_one() {
        let mut state = InterruptState::default();
        assert_eq!(state.read_if(), 0xE0);
        state.write_if(0xFF);
        assert_eq!(state.requested, 0x1F, "only 5 bits are stored");
        assert_eq!(state.read_if(), 0xFF);
    }
}
