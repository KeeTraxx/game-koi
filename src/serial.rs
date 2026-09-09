//! The serial port: SB and SC.
//!
//! Two registers, per chapter 11 of the reference:
//!
//! - **SB** (0xFF01) — the 8-bit shift register holding the byte being transferred.
//! - **SC** (0xFF02) — control. `SIO_EN` (bit 7) starts a transfer; `SIO_CLK` (bit 0)
//!   selects whether this Game Boy supplies the clock (internal, 1) or waits for the
//!   other end to supply it (external, 0). Bits 1-6 are unimplemented.
//!
//! # Why this exists before the PPU
//!
//! Nothing is plugged into the link port, so real hardware would stall forever
//! waiting for a peer. But the *reason* to implement this now is that the community
//! test ROMs report their results through it: Blargg's suite writes each character
//! of "Passed" or "Failed" to SB and pokes SC to transmit. Collecting those bytes
//! turns a test ROM into a pass/fail signal, which is the first external validation
//! of the CPU and timer.
//!
//! # What an unplugged link port does
//!
//! With no peer, each transferred bit shifts in a 1, so a completed transfer leaves
//! SB reading 0xFF. We honour that. A transfer using the *external* clock never
//! completes at all, since nothing is driving the clock — so those are left pending
//! forever, which is what hardware does.

use crate::interrupts::{Interrupt, InterruptState};

/// Cycles for a full 8-bit transfer at the normal serial rate.
///
/// The link runs at 8192 bits/second, so one bit takes 512 T-cycles = 128 M-cycles,
/// and a byte takes eight of those.
const M_CYCLES_PER_BIT: u32 = 128;

pub struct Serial {
    /// SB (0xFF01).
    data: u8,
    /// SC (0xFF02), stored as written; only bits 7 and 0 mean anything.
    control: u8,
    /// M-cycles remaining in the bit currently being shifted, if a transfer is
    /// running on our own clock.
    bit_timer: u32,
    /// Bits still to shift in the current transfer.
    bits_left: u8,
    /// Every byte the program has transmitted, in order.
    ///
    /// This is not hardware — it is the tap that makes test ROMs readable. Real
    /// hardware would just shift the bits out of the port and forget them.
    output: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Serial {
            data: 0x00,
            control: 0x00,
            bit_timer: 0,
            bits_left: 0,
            output: Vec::new(),
        }
    }

    /// True while a transfer is in progress.
    fn transfer_requested(&self) -> bool {
        self.control & 0x80 != 0
    }

    /// True if this Game Boy supplies the clock. With no peer attached, a transfer
    /// on an external clock never advances.
    fn internal_clock(&self) -> bool {
        self.control & 0x01 != 0
    }

    /// Advances the port by one M-cycle.
    pub fn tick(&mut self, interrupts: &mut InterruptState) {
        if !self.transfer_requested() || !self.internal_clock() || self.bits_left == 0 {
            return;
        }

        self.bit_timer -= 1;
        if self.bit_timer > 0 {
            return;
        }

        // One bit period elapsed: shift a bit out and a 1 in, since nothing is
        // driving the other end of the wire.
        self.data = (self.data << 1) | 1;
        self.bits_left -= 1;
        self.bit_timer = M_CYCLES_PER_BIT;

        if self.bits_left == 0 {
            // The transfer is complete: hardware clears the enable bit and raises
            // the serial interrupt.
            self.control &= !0x80;
            interrupts.request(Interrupt::Serial);
        }
    }

    pub fn read(&self, address: u16) -> u8 {
        match address {
            0xFF01 => self.data,
            // The six unimplemented bits read back as 1, matching how other
            // partially-implemented registers behave.
            0xFF02 => self.control | 0x7E,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0xFF01 => self.data = value,
            0xFF02 => {
                self.control = value & 0x81;
                if self.transfer_requested() {
                    // Capture the byte as it was handed to us. Doing it at the start
                    // of the transfer rather than the end matters: by the time the
                    // transfer finishes, SB has been shifted to 0xFF and the
                    // original byte is gone.
                    self.output.push(self.data);
                    self.bits_left = 8;
                    self.bit_timer = M_CYCLES_PER_BIT;
                }
            }
            _ => {}
        }
    }

    /// Everything written to the port so far.
    #[cfg(test)]
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// The transmitted bytes as text, for reading test ROM results.
    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
}

impl Default for Serial {
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

    /// Writes a byte the way a test ROM does: SB, then SC with the start bit.
    fn transmit(serial: &mut Serial, byte: u8) {
        serial.write(0xFF01, byte);
        serial.write(0xFF02, 0x81);
    }

    fn tick_n(serial: &mut Serial, ints: &mut InterruptState, n: u32) {
        for _ in 0..n {
            serial.tick(ints);
        }
    }

    #[test]
    fn captures_written_bytes() {
        let mut serial = Serial::new();
        let mut ints = interrupts();
        for byte in b"Hi" {
            transmit(&mut serial, *byte);
            tick_n(&mut serial, &mut ints, M_CYCLES_PER_BIT * 8);
        }
        assert_eq!(serial.output(), b"Hi");
        assert_eq!(serial.output_text(), "Hi");
    }

    #[test]
    fn capture_happens_at_transfer_start() {
        // The byte is recorded when the transfer begins, because SB is destroyed by
        // the shifting before the transfer ends.
        let mut serial = Serial::new();
        let mut ints = interrupts();
        transmit(&mut serial, b'A');
        assert_eq!(serial.output(), b"A", "captured immediately");

        tick_n(&mut serial, &mut ints, M_CYCLES_PER_BIT * 8);
        assert_eq!(serial.read(0xFF01), 0xFF, "SB shifted full of ones");
        assert_eq!(serial.output(), b"A", "but the captured byte survives");
    }

    #[test]
    fn transfer_takes_eight_bit_periods() {
        let mut serial = Serial::new();
        let mut ints = interrupts();
        transmit(&mut serial, 0x00);

        tick_n(&mut serial, &mut ints, M_CYCLES_PER_BIT * 8 - 1);
        assert!(!ints.any_pending(), "not finished yet");
        assert_eq!(serial.read(0xFF02) & 0x80, 0x80, "still transferring");

        tick_n(&mut serial, &mut ints, 1);
        assert_eq!(ints.pending(), Some(Interrupt::Serial));
        assert_eq!(
            serial.read(0xFF02) & 0x80,
            0,
            "enable bit cleared by hardware"
        );
    }

    #[test]
    fn external_clock_transfers_never_complete() {
        // SC bit 0 clear means the peer supplies the clock. With nothing plugged in,
        // the transfer hangs forever — which is what real hardware does.
        let mut serial = Serial::new();
        let mut ints = interrupts();
        serial.write(0xFF01, b'X');
        serial.write(0xFF02, 0x80); // enable, external clock

        tick_n(&mut serial, &mut ints, M_CYCLES_PER_BIT * 100);
        assert!(!ints.any_pending(), "no clock, no completion");
        assert_eq!(serial.read(0xFF02) & 0x80, 0x80, "still pending");
    }

    #[test]
    fn unused_control_bits_read_as_one() {
        let mut serial = Serial::new();
        serial.write(0xFF02, 0x00);
        assert_eq!(serial.read(0xFF02), 0x7E);
        // Only bits 7 and 0 are stored.
        serial.write(0xFF02, 0xFF);
        assert_eq!(serial.read(0xFF02), 0xFF);
    }

    #[test]
    fn writing_sb_without_starting_transmits_nothing() {
        let mut serial = Serial::new();
        let mut ints = interrupts();
        serial.write(0xFF01, b'Z');
        tick_n(&mut serial, &mut ints, M_CYCLES_PER_BIT * 8);
        assert!(serial.output().is_empty(), "no transfer was requested");
    }
}
