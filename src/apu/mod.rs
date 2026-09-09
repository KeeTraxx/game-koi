//! The APU: four sound channels, a mixer, and the counters that drive them.
//!
//! > The PPU is a bunch of state machines, and the APU is a bunch of counters.
//! >  — Pandocs
//!
//! That is the whole design. Each channel has a frequency timer counting down to its
//! next sample, and a set of slower counters — length, envelope, sweep — clocked by a
//! shared 512 Hz sequencer. Nothing here generates a waveform mathematically; it all
//! falls out of counters reloading.
//!
//! # The DIV-APU sequencer
//!
//! The 512 Hz clock is not a separate oscillator. It is bit 4 of the `DIV` register
//! going from 1 to 0 — the same divider the timer uses, tapped further up. That has a
//! consequence worth keeping: writing to `DIV` resets it, and if bit 4 happened to be
//! set, the write *manufactures a falling edge* and steps the sequencer early. Games can
//! and do make length counters tick faster this way. Because the APU watches DIV rather
//! than counting for itself, that behavior is free here rather than a special case.
//!
//! | Sequencer step | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
//! |---|---|---|---|---|---|---|---|---|
//! | Length (256 Hz) | x | | x | | x | | x | |
//! | Sweep (128 Hz) | | | x | | | | x | |
//! | Envelope (64 Hz) | | | | | | | | x |
//!
//! # Digital, then analog
//!
//! Each channel produces a digital 0-15. A DAC per channel turns that into an analog
//! level, and only then does the mixer add them up — which is why a *disabled* channel
//! with a *powered* DAC is not silent, it is a constant DC offset. Real hardware has a
//! high-pass filter to drag that back to zero, and so does this: without it, muting a
//! channel would leave an audible step in the output.

mod noise;
mod square;
mod units;
mod wave;

use std::collections::VecDeque;

use noise::Noise;
use square::Square;
use wave::Wave;

/// Host sample rate, in Hz. Overridable, because the audio device picks the real one.
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// The APU is clocked at the M-cycle rate: 4194304 / 4.
const M_CYCLES_PER_SECOND: f64 = 1_048_576.0;

/// How many stereo samples to keep before dropping the oldest.
///
/// Roughly a fifth of a second at 48 kHz. The frontend runs a frame's worth of emulation
/// and then sleeps, so production is bursty and the queue has to absorb that; but an
/// unbounded queue would just grow forever if the audio device were slower than the
/// emulator, adding latency without ever recovering.
const MAX_QUEUED_SAMPLES: usize = 8192;

/// What each register reads back as, ORed over the stored value.
///
/// Write-only fields read as ones, because nothing drives those bus lines. This is the
/// table Blargg's `dmg_sound` test 01 checks byte by byte, and it is much easier to get
/// right as data than as logic scattered through the channels.
const READ_MASKS: [u8; 0x17] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10-NR14
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, // NR20-NR24 (NR20 does not exist)
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30-NR34
    0xFF, 0xFF, 0x00, 0x00, 0xBF, // NR40-NR44 (NR40 does not exist)
    0x00, 0x00, 0x70, // NR50-NR52
];

pub struct Apu {
    channel1: Square,
    channel2: Square,
    channel3: Wave,
    channel4: Noise,

    /// NR50: master volume per side, plus the VIN mixing bits no cartridge ever used.
    nr50: u8,
    /// NR51: which channels reach which side.
    nr51: u8,
    /// NR52 bit 7. While clear, every other register is zeroed and read-only.
    powered: bool,

    /// Sequencer step, 0-7.
    sequencer_step: u8,
    /// Last seen state of DIV bit 4, for the falling-edge detector.
    previous_div_bit: bool,

    /// Fractional accumulator for turning the 1048576 Hz stream into host samples.
    sample_clock: f64,
    cycles_per_sample: f64,
    /// Running sum of the mixed output since the last emitted sample, and how many
    /// M-cycles went into it. Averaging over the interval rather than picking one value
    /// out of every twenty-odd is a cheap low-pass, and audibly less harsh.
    accumulator: (f32, f32),
    accumulated: u32,

    /// The high-pass filter's state, one per side. See the module docs.
    capacitor: (f32, f32),
    charge_factor: f32,

    /// Finished stereo samples, waiting for the host to collect them.
    samples: VecDeque<(f32, f32)>,
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Apu {
            channel1: Square::new(true),
            channel2: Square::new(false),
            channel3: Wave::new(),
            channel4: Noise::new(),
            nr50: 0,
            nr51: 0,
            powered: false,
            sequencer_step: 0,
            previous_div_bit: false,
            sample_clock: 0.0,
            cycles_per_sample: 0.0,
            accumulator: (0.0, 0.0),
            accumulated: 0,
            capacitor: (0.0, 0.0),
            charge_factor: 0.0,
            samples: VecDeque::new(),
        };
        apu.set_sample_rate(DEFAULT_SAMPLE_RATE);
        apu
    }

    /// Retunes the resampler and the filter to the host's actual output rate.
    pub fn set_sample_rate(&mut self, rate: u32) {
        let rate = rate.max(1);
        self.cycles_per_sample = M_CYCLES_PER_SECOND / rate as f64;
        // The per-T-cycle charge factor from Pandocs, raised to the number of T-cycles
        // one output sample covers — so the filter's cutoff stays put whatever rate the
        // device runs at.
        self.charge_factor = 0.999958f32.powf(4_194_304.0 / rate as f32);
    }

    /// Takes every finished sample, leaving the queue empty.
    pub fn drain_samples(&mut self, out: &mut Vec<(f32, f32)>) {
        out.extend(self.samples.drain(..));
    }

    /// Advances the APU by one M-cycle. `div` is the timer's DIV register, which is
    /// where the 512 Hz sequencer clock comes from.
    pub fn tick(&mut self, div: u8) {
        // The sequencer keeps running with the APU powered off — it is part of the
        // timer, not the sound hardware, so it never stops.
        let div_bit = div & 0x10 != 0;
        if self.previous_div_bit && !div_bit {
            self.step_sequencer();
        }
        self.previous_div_bit = div_bit;

        if self.powered {
            self.channel1.tick();
            self.channel2.tick();
            // Twice: the wave channel's divider runs at 2097152 Hz, double the others.
            self.channel3.tick();
            self.channel3.tick();
            self.channel4.tick();
        }

        self.collect_sample();
    }

    /// Whether the sequencer's *next* step will skip the length counters.
    ///
    /// Length is clocked on the even steps, so this is true during the odd half of the
    /// 512 Hz period. A write to NRx4 in that half behaves differently — see
    /// [`units::LengthTimer::extra_clock`].
    fn extra_length_clock(&self) -> bool {
        !self.sequencer_step.is_multiple_of(2)
    }

    fn step_sequencer(&mut self) {
        // The step advances whether or not the APU is powered, because there is no
        // separate counter to stop — it is derived from DIV, and DIV keeps running.
        // Only the units it drives are switched off with the power.
        let step = self.sequencer_step;
        self.sequencer_step = (step + 1) % 8;

        if !self.powered {
            return;
        }

        // Length on the even steps, sweep on 2 and 6, envelope on 7. The phase matters:
        // it is why a length counter enabled on an odd step gets an extra tick.
        if step.is_multiple_of(2) {
            self.channel1.tick_length();
            self.channel2.tick_length();
            self.channel3.tick_length();
            self.channel4.tick_length();
        }
        if step == 2 || step == 6 {
            self.channel1.tick_sweep();
        }
        if step == 7 {
            self.channel1.tick_envelope();
            self.channel2.tick_envelope();
            self.channel4.tick_envelope();
        }
    }

    /// Runs one channel's digital output through its DAC.
    ///
    /// A powered DAC maps 0-15 onto the analog range; an unpowered one contributes
    /// nothing at all. On real hardware the slope is negative — digital 0 becomes analog
    /// *1* — but that only inverts the phase of the whole signal, which is inaudible, so
    /// the conventional positive slope is used here for readability.
    fn dac(digital: u8, enabled: bool) -> f32 {
        if !enabled {
            return 0.0;
        }
        digital as f32 / 7.5 - 1.0
    }

    /// Mixes the four channels into a stereo pair, before the high-pass filter.
    fn mix(&self) -> (f32, f32) {
        if !self.powered {
            return (0.0, 0.0);
        }

        let channels = [
            Self::dac(self.channel1.output(), self.channel1.dac_enabled()),
            Self::dac(self.channel2.output(), self.channel2.dac_enabled()),
            Self::dac(self.channel3.output(), self.channel3.dac_enabled()),
            Self::dac(self.channel4.output(), self.channel4.dac_enabled()),
        ];

        // NR51's low nibble is the right side, its high nibble the left.
        let mut left = 0.0;
        let mut right = 0.0;
        for (i, sample) in channels.iter().enumerate() {
            if self.nr51 & (1 << (i + 4)) != 0 {
                left += sample;
            }
            if self.nr51 & (1 << i) != 0 {
                right += sample;
            }
        }

        // Master volume is 0-7 meaning a scale of 1-8, so it can never mute a signal —
        // only NR51 can do that.
        let left_volume = ((self.nr50 >> 4) & 0x07) as f32 + 1.0;
        let right_volume = (self.nr50 & 0x07) as f32 + 1.0;

        // Divide by the maximum: four channels at full deflection times a volume of 8.
        (left * left_volume / 32.0, right * right_volume / 32.0)
    }

    /// Accumulates this M-cycle's output and emits a host sample when one is due.
    fn collect_sample(&mut self) {
        let (left, right) = self.mix();
        self.accumulator.0 += left;
        self.accumulator.1 += right;
        self.accumulated += 1;

        self.sample_clock += 1.0;
        if self.sample_clock < self.cycles_per_sample {
            return;
        }
        self.sample_clock -= self.cycles_per_sample;

        let count = self.accumulated as f32;
        let averaged = (self.accumulator.0 / count, self.accumulator.1 / count);
        self.accumulator = (0.0, 0.0);
        self.accumulated = 0;

        let filtered = (
            self.high_pass(averaged.0, false),
            self.high_pass(averaged.1, true),
        );

        if self.samples.len() >= MAX_QUEUED_SAMPLES {
            // Drop the oldest rather than the newest: if we have fallen behind, the
            // stale audio is the part nobody wants to hear.
            self.samples.pop_front();
        }
        self.samples.push_back(filtered);
    }

    /// Removes the DC offset that idle-but-powered DACs leave behind.
    ///
    /// The capacitor charges toward the input, and the output is the difference — so a
    /// constant input decays to zero while a changing one passes through. Without this,
    /// switching a channel's DAC on would shift the whole waveform off centre and stay
    /// there.
    fn high_pass(&mut self, input: f32, right: bool) -> f32 {
        let capacitor = if right {
            &mut self.capacitor.1
        } else {
            &mut self.capacitor.0
        };
        let output = input - *capacitor;
        *capacitor = input - output * self.charge_factor;
        output
    }

    pub fn read(&self, address: u16) -> u8 {
        match address {
            0xFF10..=0xFF26 => {
                let index = (address - 0xFF10) as usize;
                let value = match address {
                    0xFF10..=0xFF14 => self.channel1.read((address - 0xFF10) as u8),
                    0xFF15..=0xFF19 => self.channel2.read((address - 0xFF15) as u8),
                    0xFF1A..=0xFF1E => self.channel3.read((address - 0xFF1A) as u8),
                    0xFF1F..=0xFF23 => self.channel4.read((address - 0xFF1F) as u8),
                    0xFF24 => self.nr50,
                    0xFF25 => self.nr51,
                    0xFF26 => self.read_nr52(),
                    _ => 0,
                };
                value | READ_MASKS[index]
            }
            // Wave RAM is readable even with the APU powered off — it is just memory,
            // and games use it as scratch space. It is only locked while CH3 plays.
            0xFF30..=0xFF3F if self.channel3.ram_accessible() => {
                self.channel3.ram[(address - 0xFF30) as usize]
            }
            0xFF30..=0xFF3F => 0xFF,
            _ => 0xFF,
        }
    }

    fn read_nr52(&self) -> u8 {
        // The low four bits report the *generators*, not the DACs, and are read-only:
        // writing them does not start or stop anything.
        u8::from(self.powered) << 7
            | u8::from(self.channel4.is_enabled()) << 3
            | u8::from(self.channel3.is_enabled()) << 2
            | u8::from(self.channel2.is_enabled()) << 1
            | u8::from(self.channel1.is_enabled())
    }

    pub fn write(&mut self, address: u16, value: u8) {
        // Wave RAM ignores the power state entirely.
        if (0xFF30..=0xFF3F).contains(&address) {
            if self.channel3.ram_accessible() {
                self.channel3.ram[(address - 0xFF30) as usize] = value;
            }
            return;
        }

        // With the APU off every register but NR52 is read-only. This is not a
        // formality — a sound driver writing to a powered-down APU expects its writes to
        // vanish, and honouring them instead produces sound at the wrong time.
        //
        // The length *load* registers are the DMG's exception: their counters survive a
        // power cycle and can still be loaded while the APU is off. Only the length
        // field, though — the duty bits sharing NR11 and NR21 are ignored. Blargg's
        // `11-regs after power` exists to catch exactly this.
        if !self.powered {
            match address {
                0xFF11 => self.channel1.write_length(value),
                0xFF16 => self.channel2.write_length(value),
                0xFF1B => self.channel3.write_length(value),
                0xFF20 => self.channel4.write_length(value),
                0xFF26 => self.set_power(value & 0x80 != 0),
                _ => {}
            }
            return;
        }

        let extra = self.extra_length_clock();
        match address {
            0xFF10..=0xFF14 => self.channel1.write((address - 0xFF10) as u8, value, extra),
            0xFF15..=0xFF19 => self.channel2.write((address - 0xFF15) as u8, value, extra),
            0xFF1A..=0xFF1E => self.channel3.write((address - 0xFF1A) as u8, value, extra),
            0xFF1F..=0xFF23 => self.channel4.write((address - 0xFF1F) as u8, value, extra),
            0xFF24 => self.nr50 = value,
            0xFF25 => self.nr51 = value,
            0xFF26 => self.set_power(value & 0x80 != 0),
            _ => {}
        }
    }

    fn set_power(&mut self, on: bool) {
        if self.powered == on {
            return;
        }
        self.powered = on;

        if !on {
            // Powering off wipes every register. Wave RAM and the DIV-APU counter are
            // the exceptions, which is why neither is touched here.
            self.channel1.power_off();
            self.channel2.power_off();
            self.channel3.power_off();
            self.channel4.power_off();
            self.nr50 = 0;
            self.nr51 = 0;
        } else {
            // The *timing* of the next step stays tied to DIV — nothing here can move
            // it — but the step counter itself restarts, so the first thing a freshly
            // powered APU does is a length clock. Blargg's `07-len sweep period sync`
            // measures the distinction.
            self.sequencer_step = 0;
        }
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks the APU for a number of M-cycles, feeding it a DIV that advances at the
    /// real rate — one DIV increment per 64 M-cycles.
    fn run(apu: &mut Apu, cycles: u32) {
        for i in 0..cycles {
            apu.tick((i / 64) as u8);
        }
    }

    /// One full pass of the 8-step sequencer takes 8 * 2048 M-cycles.
    fn powered() -> Apu {
        let mut apu = Apu::new();
        apu.write(0xFF26, 0x80);
        apu.write(0xFF24, 0x77); // full volume both sides
        apu.write(0xFF25, 0xFF); // everything to both sides
        apu
    }

    #[test]
    fn registers_read_back_with_the_write_only_bits_set() {
        let mut apu = powered();
        apu.write(0xFF11, 0x3F); // duty 0, length 63
        // The length field is write-only, so only the duty bits come back.
        assert_eq!(apu.read(0xFF11), 0x3F, "duty 0, plus the write-only mask");

        apu.write(0xFF11, 0xC0); // duty 3
        assert_eq!(apu.read(0xFF11), 0xC0 | 0x3F);

        // NR13 is entirely write-only.
        apu.write(0xFF13, 0x12);
        assert_eq!(apu.read(0xFF13), 0xFF);
    }

    #[test]
    fn nonexistent_registers_read_as_ones() {
        let apu = powered();
        assert_eq!(apu.read(0xFF15), 0xFF, "NR20 does not exist");
        assert_eq!(apu.read(0xFF1F), 0xFF, "NR40 does not exist");
    }

    #[test]
    fn powering_off_clears_the_registers_and_locks_them() {
        let mut apu = powered();
        apu.write(0xFF12, 0xF0);
        assert_eq!(apu.read(0xFF12), 0xF0);

        apu.write(0xFF26, 0x00);
        assert_eq!(apu.read(0xFF12), 0x00, "cleared by the power-off");

        // Writes are ignored until the power comes back.
        apu.write(0xFF12, 0xF0);
        assert_eq!(apu.read(0xFF12), 0x00);
        apu.write(0xFF26, 0x80);
        apu.write(0xFF12, 0xF0);
        assert_eq!(apu.read(0xFF12), 0xF0);
    }

    #[test]
    fn wave_ram_survives_a_power_cycle_and_ignores_the_power_state() {
        let mut apu = Apu::new();
        // Powered off, and wave RAM still takes writes.
        apu.write(0xFF30, 0xAB);
        assert_eq!(apu.read(0xFF30), 0xAB);

        apu.write(0xFF26, 0x80);
        assert_eq!(apu.read(0xFF30), 0xAB);
        apu.write(0xFF26, 0x00);
        assert_eq!(apu.read(0xFF30), 0xAB, "not cleared with the rest");
    }

    #[test]
    fn nr52_reports_channels_not_dacs() {
        let mut apu = powered();
        assert_eq!(apu.read(0xFF26) & 0x0F, 0, "nothing playing yet");

        // Powering CH1's DAC alone must not set the status bit.
        apu.write(0xFF12, 0xF0);
        assert_eq!(apu.read(0xFF26) & 0x01, 0, "DAC on is not channel on");

        apu.write(0xFF14, 0x80); // trigger
        assert_eq!(apu.read(0xFF26) & 0x01, 1);

        // And writing the status bits does nothing.
        apu.write(0xFF26, 0x80);
        assert_eq!(apu.read(0xFF26) & 0x01, 1, "still playing");
    }

    #[test]
    fn the_sequencer_follows_div_not_its_own_clock() {
        let mut apu = powered();
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF11, 0x3F); // length 63: one tick from expiry
        apu.write(0xFF14, 0xC0); // trigger with length enabled
        assert!(apu.channel1.is_enabled());

        // Two DIV bit-4 falling edges: the first is sequencer step 0, which clocks
        // length. Feed the edges directly rather than waiting out the cycles.
        apu.tick(0x10);
        apu.tick(0x00);
        assert!(!apu.channel1.is_enabled(), "one length tick was enough");
    }

    #[test]
    fn writing_div_can_step_the_sequencer_early() {
        // The falling edge is all that matters, so a DIV reset while bit 4 is set
        // manufactures one. This is a real trick games use to speed up length counters.
        let mut apu = powered();
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF11, 0x3E); // length 62: two ticks
        apu.write(0xFF14, 0xC0);

        // Four manufactured edges rather than the ~8192 cycles two length ticks would
        // take at the real rate. Four, not two, because only the even steps clock
        // length — the sequencer's phase is as real as its rate.
        for _ in 0..4 {
            apu.tick(0x10);
            apu.tick(0x00);
        }
        assert!(!apu.channel1.is_enabled());
    }

    #[test]
    fn the_sequencer_runs_with_the_apu_powered_off() {
        // It lives in the timer, not the sound hardware, so the step keeps advancing.
        // Nothing observable happens because the units are off — but this is why
        // powering the APU back on does not resynchronise the DIV-APU counter.
        let mut apu = Apu::new();
        apu.tick(0x10);
        apu.tick(0x00);
        assert_eq!(
            apu.sequencer_step, 1,
            "the step advances with the power off"
        );

        // Powering on restarts the count, but cannot move when the next edge arrives:
        // that is DIV's business.
        apu.write(0xFF26, 0x80);
        assert_eq!(apu.sequencer_step, 0);
    }

    #[test]
    fn a_silent_apu_produces_silence() {
        let mut apu = powered();
        run(&mut apu, 100_000);
        let mut samples = Vec::new();
        apu.drain_samples(&mut samples);
        assert!(!samples.is_empty(), "samples are produced regardless");
        assert!(
            samples
                .iter()
                .all(|(l, r)| l.abs() < 1e-6 && r.abs() < 1e-6),
            "no channel is playing, so the output must be silent"
        );
    }

    #[test]
    fn a_playing_channel_produces_a_signal() {
        let mut apu = powered();
        apu.write(0xFF11, 0x80); // 50% duty
        apu.write(0xFF12, 0xF0); // full volume
        apu.write(0xFF13, 0x00);
        apu.write(0xFF14, 0x84); // period 0x400, trigger

        run(&mut apu, 200_000);
        let mut samples = Vec::new();
        apu.drain_samples(&mut samples);

        let peak = samples.iter().map(|(l, _)| l.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.05, "expected an audible signal, peak was {peak}");
    }

    #[test]
    fn the_high_pass_filter_removes_a_constant_offset() {
        // A powered DAC on a silent channel is a DC offset, which must decay away
        // rather than sitting in the output forever.
        let mut apu = powered();
        apu.write(0xFF12, 0x08); // DAC on, volume 0, channel never triggered
        run(&mut apu, 400_000);

        let mut samples = Vec::new();
        apu.drain_samples(&mut samples);
        let last = samples.last().copied().unwrap_or((0.0, 0.0));
        assert!(
            last.0.abs() < 0.01,
            "the offset should have decayed, got {}",
            last.0
        );
    }

    #[test]
    fn the_sample_queue_is_bounded() {
        let mut apu = powered();
        // Far more audio than anyone drained.
        run(&mut apu, 5_000_000);
        assert!(apu.samples.len() <= MAX_QUEUED_SAMPLES);
    }

    #[test]
    fn panning_routes_channels_to_sides() {
        let mut apu = powered();
        apu.write(0xFF25, 0x10); // CH1 left only
        apu.write(0xFF11, 0x80);
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF13, 0x00);
        apu.write(0xFF14, 0x84);

        run(&mut apu, 200_000);
        let mut samples = Vec::new();
        apu.drain_samples(&mut samples);

        let left_peak = samples.iter().map(|(l, _)| l.abs()).fold(0.0f32, f32::max);
        let right_peak = samples.iter().map(|(_, r)| r.abs()).fold(0.0f32, f32::max);
        assert!(left_peak > 0.05, "left should hear it");
        assert!(right_peak < 1e-6, "right should not");
    }
}
