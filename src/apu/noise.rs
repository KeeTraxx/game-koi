//! Channel 4: white noise from a linear-feedback shift register.
//!
//! There is no wavetable and no oscillator. A 15-bit shift register is clocked, and the
//! bit that falls out the bottom decides whether the channel emits its volume or
//! silence. Feedback from the bottom two bits makes the sequence pseudo-random and very
//! long, so it sounds like noise rather than a tone.
//!
//! Switching the register to 7 bits shortens the sequence enough that the ear hears it
//! repeat — the result is a buzzy, metallic pitch rather than a hiss. That is a feature,
//! not a defect: it is how games get snare drums out of the same channel as cymbals.

use super::units::{Envelope, LengthTimer};

pub(super) struct Noise {
    /// NR43 as written. Kept whole so reads give it back exactly.
    control: u8,
    /// 15 bits of state. Reset to 0 on trigger, and the feedback fills it from there.
    lfsr: u16,
    /// Counts down in M-cycles to the next shift.
    timer: u32,

    length: LengthTimer,
    envelope: Envelope,
    enabled: bool,
}

impl Noise {
    pub(super) fn new() -> Self {
        Noise {
            control: 0,
            lfsr: 0,
            timer: 1,
            length: LengthTimer::new(64),
            envelope: Envelope::new(),
            enabled: false,
        }
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn dac_enabled(&self) -> bool {
        self.envelope.dac_enabled()
    }

    pub(super) fn output(&self) -> u8 {
        if !self.enabled {
            return 0;
        }
        // Bit 0 is the bit about to fall out. A 1 means silence — the sense is
        // inverted, which is why a freshly triggered (all-zero) register starts loud.
        if self.lfsr & 1 == 0 {
            self.envelope.volume()
        } else {
            0
        }
    }

    fn shift(&self) -> u8 {
        self.control >> 4
    }

    fn short_mode(&self) -> bool {
        self.control & 0x08 != 0
    }

    fn divider(&self) -> u8 {
        self.control & 0x07
    }

    /// How many M-cycles between shifts.
    ///
    /// The LFSR is clocked at 262144 / (divider x 2^shift) Hz, and the M-cycle clock is
    /// 1048576 Hz, so the base period is 4 M-cycles. A divider of 0 means one *half*,
    /// giving 2.
    fn period(&self) -> u32 {
        let base: u32 = if self.divider() == 0 {
            2
        } else {
            self.divider() as u32 * 4
        };
        base << self.shift()
    }

    pub(super) fn tick(&mut self) {
        // Shifts of 14 and 15 stop the register being clocked at all — the channel
        // freezes on whatever bit it last shifted out rather than going silent.
        if self.shift() >= 14 {
            return;
        }

        self.timer -= 1;
        if self.timer != 0 {
            return;
        }
        self.timer = self.period();

        // The new bit is the XNOR of the bottom two: 1 when they agree.
        let feedback = ((self.lfsr ^ (self.lfsr >> 1)) & 1) ^ 1;
        self.lfsr = (self.lfsr >> 1) | (feedback << 14);
        if self.short_mode() {
            // In 7-bit mode the feedback is also written to bit 6, which folds the
            // sequence down to 127 steps.
            self.lfsr = (self.lfsr & !(1 << 6)) | (feedback << 6);
        }
    }

    pub(super) fn tick_length(&mut self) {
        if self.length.tick() {
            self.enabled = false;
        }
    }

    pub(super) fn tick_envelope(&mut self) {
        self.envelope.tick();
    }

    pub(super) fn read(&self, register: u8) -> u8 {
        match register {
            // NR40 does not exist, and NR41's length is write-only.
            0 | 1 => 0xFF,
            2 => self.envelope.read(),
            3 => self.control,
            4 => u8::from(self.length.is_enabled()) << 6,
            _ => 0xFF,
        }
    }

    /// Loads just the length field; see [`super::square::Square::write_length`].
    pub(super) fn write_length(&mut self, value: u8) {
        self.length.set((value & 0x3F) as u16);
    }

    pub(super) fn write(&mut self, register: u8, value: u8, extra_length_clock: bool) {
        match register {
            1 => self.length.set((value & 0x3F) as u16),
            2 => {
                self.envelope.write(value);
                if !self.envelope.dac_enabled() {
                    self.enabled = false;
                }
            }
            3 => self.control = value,
            4 => {
                let was_enabled = self.length.is_enabled();
                let now_enabled = value & 0x40 != 0;
                self.length.set_enabled(now_enabled);

                if extra_length_clock
                    && !was_enabled
                    && now_enabled
                    && self.length.extra_clock()
                    && value & 0x80 == 0
                {
                    self.enabled = false;
                }

                if value & 0x80 != 0 {
                    self.trigger(extra_length_clock);
                }
            }
            _ => {}
        }
    }

    fn trigger(&mut self, extra_length_clock: bool) {
        self.enabled = self.envelope.dac_enabled();
        self.length.trigger(extra_length_clock);
        self.envelope.trigger();
        self.timer = self.period();
        // Clearing the register is what makes a retrigger sound identical every time,
        // and what rescues a register that has locked up in short mode.
        self.lfsr = 0;
    }

    pub(super) fn power_off(&mut self) {
        self.control = 0;
        self.lfsr = 0;
        self.enabled = false;
        self.length.power_off();
        self.envelope.power_off();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(control: u8) -> Noise {
        let mut noise = Noise::new();
        noise.write(2, 0xF0, false); // volume 15, no envelope
        noise.write(3, control, false);
        noise.write(4, 0x80, false); // trigger
        noise
    }

    /// Collects the channel's output bit for `count` shifts.
    fn bits(noise: &mut Noise, count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..count {
            out.push(u8::from(noise.output() > 0));
            let period = noise.period();
            for _ in 0..period {
                noise.tick();
            }
        }
        out
    }

    #[test]
    fn the_divider_and_shift_set_the_rate() {
        // Base rate: divider 1, shift 0 is one shift per 4 M-cycles.
        let noise = playing(0x10);
        assert_eq!(noise.period(), 4);

        // Divider 0 counts as a half, so it is twice as fast.
        let noise = playing(0x00);
        assert_eq!(noise.period(), 2);

        // Each shift step doubles the period.
        let noise = playing(0x21); // shift 2, divider 1
        assert_eq!(noise.period(), 16);
    }

    #[test]
    fn fifteen_bit_mode_does_not_repeat_quickly() {
        let mut noise = playing(0x10);
        let sequence = bits(&mut noise, 200);
        // A 127-step sequence would line up with itself here; a 32767-step one will not.
        let repeats = sequence[..100] == sequence[100..200];
        assert!(!repeats, "15-bit noise must not repeat every 100 shifts");
    }

    #[test]
    fn seven_bit_mode_repeats_every_127_shifts() {
        let mut noise = playing(0x18); // short mode
        let sequence = bits(&mut noise, 254);
        assert_eq!(
            sequence[..127],
            sequence[127..254],
            "the short register folds the sequence to 127 steps"
        );
    }

    #[test]
    fn a_freshly_triggered_channel_starts_loud() {
        // The register is cleared on trigger and bit 0 low means "emit the volume".
        let noise = playing(0x10);
        assert_eq!(noise.output(), 15);
    }

    #[test]
    fn a_shift_of_fourteen_freezes_the_register() {
        let mut noise = playing(0xE0); // shift 14
        let before = noise.lfsr;
        for _ in 0..100_000 {
            noise.tick();
        }
        assert_eq!(noise.lfsr, before, "no clocks reach the register");
    }

    #[test]
    fn the_envelope_scales_the_loud_half() {
        let mut noise = Noise::new();
        noise.write(2, 0x80, false); // volume 8
        noise.write(3, 0x10, false);
        noise.write(4, 0x80, false);
        assert_eq!(noise.output(), 8);
    }

    #[test]
    fn the_length_timer_stops_the_channel() {
        let mut noise = Noise::new();
        noise.write(2, 0xF0, false);
        noise.write(1, 0x3F, false);
        noise.write(4, 0xC0, false); // trigger, length enabled
        assert!(noise.is_enabled());
        noise.tick_length();
        assert!(!noise.is_enabled());
    }
}
