//! Channel 3: plays back 32 four-bit samples from wave RAM.
//!
//! The only channel that can make an arbitrary waveform, and the only one with no
//! envelope — its volume control is three coarse steps plus mute, implemented by
//! *shifting the digital sample right* rather than by scaling an analog level. That
//! detail matters when a game changes the level mid-note: shifting biases the values
//! toward 0, which the high-pass filter then has to drag back out.
//!
//! Its period divider runs at 2097152 Hz, twice the pulse channels' rate, and its
//! waveform is 32 samples rather than 8. Same period value, two octaves lower.

/// Wave RAM: 16 bytes, two 4-bit samples each, high nibble first.
const WAVE_RAM_SIZE: usize = 16;

pub(super) struct Wave {
    /// NR30 bit 7. Unlike the other channels, the DAC here has its own switch instead
    /// of being inferred from the volume register.
    dac_enabled: bool,
    /// NR32 bits 6-5: 0 mutes, 1 is full volume, 2 and 3 shift right once and twice.
    output_level: u8,
    period: u16,
    /// Counts down at 2097152 Hz — two ticks per M-cycle.
    timer: u16,
    /// Which of the 32 nibbles is next.
    index: u8,
    /// The last nibble read. The channel emits *this*, continuously, rather than
    /// reading wave RAM on demand — which is why triggering the channel replays the
    /// previous sample until the next read comes around.
    sample_buffer: u8,

    length: super::units::LengthTimer,
    enabled: bool,

    pub(super) ram: [u8; WAVE_RAM_SIZE],
}

impl Wave {
    pub(super) fn new() -> Self {
        Wave {
            dac_enabled: false,
            output_level: 0,
            period: 0,
            timer: 1,
            index: 0,
            sample_buffer: 0,
            // 256 rather than 64: NR31 is a full byte, so this channel can hold a note
            // four times as long as the others.
            length: super::units::LengthTimer::new(256),
            enabled: false,
            ram: [0; WAVE_RAM_SIZE],
        }
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn dac_enabled(&self) -> bool {
        self.dac_enabled
    }

    pub(super) fn output(&self) -> u8 {
        if !self.enabled {
            return 0;
        }
        match self.output_level {
            0 => 0,
            level => self.sample_buffer >> (level - 1),
        }
    }

    /// Advances the divider by one of its own ticks; the APU calls this twice per
    /// M-cycle.
    pub(super) fn tick(&mut self) {
        self.timer -= 1;
        if self.timer != 0 {
            return;
        }
        self.timer = 2048 - self.period;

        // Advance first, then read: this is why sample 0 is skipped when the channel
        // starts and the first nibble heard is index 1.
        self.index = (self.index + 1) % 32;
        let byte = self.ram[self.index as usize / 2];
        self.sample_buffer = if self.index.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 0x0F
        };
    }

    pub(super) fn tick_length(&mut self) {
        if self.length.tick() {
            self.enabled = false;
        }
    }

    pub(super) fn read(&self, register: u8) -> u8 {
        match register {
            0 => u8::from(self.dac_enabled) << 7,
            // NR31 is write-only.
            1 => 0xFF,
            2 => self.output_level << 5,
            3 => 0xFF,
            4 => u8::from(self.length.is_enabled()) << 6,
            _ => 0xFF,
        }
    }

    /// Loads just the length field; see [`super::square::Square::write_length`].
    pub(super) fn write_length(&mut self, value: u8) {
        self.length.set(value as u16);
    }

    pub(super) fn write(&mut self, register: u8, value: u8, extra_length_clock: bool) {
        match register {
            0 => {
                self.dac_enabled = value & 0x80 != 0;
                if !self.dac_enabled {
                    self.enabled = false;
                }
            }
            1 => self.length.set(value as u16),
            2 => self.output_level = (value >> 5) & 0x03,
            3 => self.period = (self.period & 0x0700) | value as u16,
            4 => {
                self.period = (self.period & 0x00FF) | ((value as u16 & 0x07) << 8);

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
        self.enabled = self.dac_enabled;
        self.length.trigger(extra_length_clock);
        self.timer = 2048 - self.period;
        // The index resets but the sample buffer deliberately does not: the channel
        // keeps emitting the stale sample until the divider next fires.
        self.index = 0;
    }

    /// Whether the CPU may touch wave RAM right now.
    ///
    /// While the channel is playing it has the RAM to itself, so the CPU's reads return
    /// open bus and its writes are dropped. This is why games switch the DAC off before
    /// loading a new waveform.
    ///
    /// Simplified: on a real DMG the lock is not absolute. The CPU *can* reach wave RAM
    /// on the exact cycle CH3 reads a sample, and what it gets is the byte CH3 is
    /// reading, whatever address it asked for. Modelling that needs the CPU's access
    /// placed *within* an M-cycle relative to the channel's, which this bus does not
    /// express — it ticks whole M-cycles with reads outside them. Blargg's `09`, `10`
    /// and `12` sound tests are the ones that notice; no game does.
    pub(super) fn ram_accessible(&self) -> bool {
        !self.enabled
    }

    pub(super) fn power_off(&mut self) {
        self.dac_enabled = false;
        self.output_level = 0;
        self.period = 0;
        self.index = 0;
        self.enabled = false;
        self.length.power_off();
        // Cleared on power-off, so a freshly powered channel emits silence rather than
        // whatever the last song left behind. Wave RAM itself survives.
        self.sample_buffer = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel loaded with a ramp: sample n holds the value n % 16.
    fn ramp() -> Wave {
        let mut wave = Wave::new();
        for (i, byte) in wave.ram.iter_mut().enumerate() {
            let high = (i * 2) % 16;
            let low = (i * 2 + 1) % 16;
            *byte = (high as u8) << 4 | low as u8;
        }
        wave.write(0, 0x80, false); // DAC on
        wave.write(2, 0x20, false); // full volume
        wave.write(3, 0x00, false);
        wave.write(4, 0x87, false); // period 0x700, trigger
        wave
    }

    fn advance_one_sample(wave: &mut Wave) {
        for _ in 0..(2048 - 0x700) {
            wave.tick();
        }
    }

    #[test]
    fn playback_starts_at_sample_one() {
        // Not sample 0: the index advances before the read, so the first nibble heard
        // is the low nibble of the first byte.
        let mut wave = ramp();
        advance_one_sample(&mut wave);
        assert_eq!(wave.output(), 1);
        advance_one_sample(&mut wave);
        assert_eq!(wave.output(), 2);
    }

    #[test]
    fn samples_are_read_high_nibble_first() {
        let mut wave = Wave::new();
        wave.ram[0] = 0xAB;
        wave.ram[1] = 0xCD;
        wave.write(0, 0x80, false);
        wave.write(2, 0x20, false);
        wave.write(3, 0x00, false);
        wave.write(4, 0x87, false);

        advance_one_sample(&mut wave); // index 1: low nibble of byte 0
        assert_eq!(wave.output(), 0xB);
        advance_one_sample(&mut wave); // index 2: high nibble of byte 1
        assert_eq!(wave.output(), 0xC);
        advance_one_sample(&mut wave);
        assert_eq!(wave.output(), 0xD);
    }

    #[test]
    fn the_output_level_shifts_rather_than_scales() {
        let mut wave = ramp();
        for _ in 0..8 {
            advance_one_sample(&mut wave);
        }
        let full = wave.output();
        assert_eq!(full, 8);

        wave.write(2, 0x40, false); // 50%
        assert_eq!(wave.output(), 4);
        wave.write(2, 0x60, false); // 25%
        assert_eq!(wave.output(), 2);
        wave.write(2, 0x00, false); // mute
        assert_eq!(wave.output(), 0);
    }

    #[test]
    fn the_wave_repeats_after_32_samples() {
        let mut wave = ramp();
        advance_one_sample(&mut wave);
        let first = wave.output();
        for _ in 0..32 {
            advance_one_sample(&mut wave);
        }
        assert_eq!(wave.output(), first);
    }

    #[test]
    fn triggering_replays_the_stale_sample() {
        let mut wave = ramp();
        for _ in 0..5 {
            advance_one_sample(&mut wave);
        }
        let held = wave.output();

        // The index resets, but the buffer is untouched, so the old sample keeps coming
        // out until the divider fires again.
        wave.write(4, 0x87, false);
        assert_eq!(wave.output(), held);
        advance_one_sample(&mut wave);
        assert_eq!(wave.output(), 1, "back to the start of the wave");
    }

    #[test]
    fn the_dac_switch_is_its_own_bit() {
        let mut wave = ramp();
        assert!(wave.is_enabled());
        wave.write(0, 0x00, false);
        assert!(!wave.is_enabled());

        // And a trigger with the DAC off cannot start it.
        wave.write(4, 0x87, false);
        assert!(!wave.is_enabled());
    }

    #[test]
    fn wave_ram_is_locked_while_the_channel_plays() {
        let mut wave = ramp();
        assert!(!wave.ram_accessible());
        wave.write(0, 0x00, false); // DAC off stops the channel
        assert!(wave.ram_accessible());
    }

    #[test]
    fn power_off_clears_the_buffer_but_not_the_ram() {
        let mut wave = ramp();
        advance_one_sample(&mut wave);
        assert_ne!(wave.output(), 0);

        wave.power_off();
        assert_eq!(wave.sample_buffer, 0);
        assert_eq!(wave.ram[1], 0x23, "wave RAM survives a power cycle");
    }
}
