//! The two pulse channels: a square wave with a selectable duty cycle, an envelope,
//! and — on channel 1 only — a period sweep.
//!
//! # How a square wave gets made
//!
//! There is no oscillator. A counter walks an 8-entry pattern of ones and zeros over
//! and over, and the pattern is what makes the shape: `00000001` spends one step of
//! eight high, so 12.5% duty. Changing the "frequency" changes how fast the counter
//! walks, not the pattern.
//!
//! # Period is a countdown, not a frequency
//!
//! The 11-bit value in NRx3/NRx4 is not a frequency — it is the *starting point* of an
//! up-counter that advances at 1048576 Hz and reloads when it overflows past 2047. So
//! the number of ticks per duty step is `2048 - period`, and a *larger* register value
//! means a *shorter* step and a *higher* note. Counting down from `2048 - period` to
//! zero is the same thing and reads better, which is what this does.

use super::units::{Envelope, LengthTimer};

/// The four duty patterns, one bit per step, MSB first.
///
/// 25% and 75% are the same wave with opposite phase — audibly identical on their own,
/// and the reason the hardware bothers with both is the phase difference when mixed
/// against another channel.
const DUTY_PATTERNS: [u8; 4] = [0b0000_0001, 0b1000_0001, 0b1000_0111, 0b0111_1110];

pub(super) struct Square {
    /// NRx1 bits 7-6.
    duty: u8,
    /// The 11-bit period value from NRx3 and NRx4.
    period: u16,
    /// Counts down one per M-cycle; reloads to `2048 - period` and advances the wave.
    timer: u16,
    /// Which of the eight steps of the duty pattern is current.
    step: u8,

    length: LengthTimer,
    envelope: Envelope,

    /// Whether the generator is running. Distinct from the DAC being powered: a
    /// disabled channel still feeds a digital 0 to an enabled DAC.
    enabled: bool,

    /// Channel 1 has a sweep unit; channel 2 does not. The `Option` is the difference
    /// between the two channels — everything else is shared.
    sweep: Option<Sweep>,
}

impl Square {
    pub(super) fn new(with_sweep: bool) -> Self {
        Square {
            duty: 0,
            period: 0,
            timer: 1,
            step: 0,
            length: LengthTimer::new(64),
            envelope: Envelope::new(),
            enabled: false,
            sweep: with_sweep.then(Sweep::new),
        }
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn dac_enabled(&self) -> bool {
        self.envelope.dac_enabled()
    }

    /// The channel's digital output, 0-15, before the DAC.
    pub(super) fn output(&self) -> u8 {
        if !self.enabled {
            return 0;
        }
        let high = DUTY_PATTERNS[self.duty as usize] >> (7 - self.step) & 1;
        if high == 1 { self.envelope.volume() } else { 0 }
    }

    /// Advances the frequency timer by one M-cycle.
    pub(super) fn tick(&mut self) {
        self.timer -= 1;
        if self.timer == 0 {
            // Reloading here rather than at the write is what makes a period change
            // take effect only after the sample in progress finishes.
            self.timer = 2048 - self.period;
            self.step = (self.step + 1) % 8;
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

    /// Clocked at 128 Hz. Channel 2 has no sweep and ignores this.
    pub(super) fn tick_sweep(&mut self) {
        let Some(sweep) = self.sweep.as_mut() else {
            return;
        };
        let result = sweep.tick();
        // Write back before honouring the overflow: the period really does land in
        // NR13/NR14 even on the step that stops the channel.
        if let Some(period) = result.period {
            self.period = period;
        }
        if result.overflowed {
            self.enabled = false;
        }
    }

    pub(super) fn read(&self, register: u8) -> u8 {
        match register {
            0 => self.sweep.as_ref().map_or(0xFF, Sweep::read),
            1 => self.duty << 6,
            2 => self.envelope.read(),
            // NRx3 is write-only; the period is not readable at all.
            3 => 0xFF,
            4 => u8::from(self.length.is_enabled()) << 6,
            _ => 0xFF,
        }
    }

    /// Loads just the length field. Usable with the APU powered off, unlike the rest
    /// of the register — see [`super::Apu::write`].
    pub(super) fn write_length(&mut self, value: u8) {
        self.length.set((value & 0x3F) as u16);
    }

    /// `extra_length_clock` says the sequencer's next step will *not* clock the length
    /// counters, which changes what a write to NRx4 does. The APU knows the phase; the
    /// channel does not, so it is passed in.
    pub(super) fn write(&mut self, register: u8, value: u8, extra_length_clock: bool) {
        match register {
            0 => {
                if let Some(sweep) = self.sweep.as_mut()
                    && sweep.write(value)
                {
                    // Clearing the direction bit after a subtracting calculation has
                    // happened kills the channel. The hardware has no way to undo a
                    // subtraction it has already applied, so it refuses to switch back.
                    self.enabled = false;
                }
            }
            1 => {
                self.duty = value >> 6;
                self.length.set((value & 0x3F) as u16);
            }
            2 => {
                self.envelope.write(value);
                // Cutting the DAC takes the channel with it, immediately.
                if !self.envelope.dac_enabled() {
                    self.enabled = false;
                }
            }
            3 => self.period = (self.period & 0x0700) | value as u16,
            4 => {
                self.period = (self.period & 0x00FF) | ((value as u16 & 0x07) << 8);

                let was_enabled = self.length.is_enabled();
                let now_enabled = value & 0x40 != 0;
                self.length.set_enabled(now_enabled);

                // Switching the length counter on mid-period costs it a tick. If that
                // is the tick that runs it out, the channel stops — unless this same
                // write is also a trigger, which is about to reload it anyway.
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
        // A trigger cannot start a channel whose DAC is off. Everything else about the
        // trigger still happens; only the enable is withheld.
        self.enabled = self.envelope.dac_enabled();
        self.length.trigger(extra_length_clock);
        self.envelope.trigger();
        self.timer = 2048 - self.period;

        if let Some(sweep) = self.sweep.as_mut()
            && sweep.trigger(self.period)
        {
            // The immediate overflow check on trigger can shut the channel off before
            // it has produced a single sample.
            self.enabled = false;
        }
    }

    pub(super) fn power_off(&mut self) {
        self.duty = 0;
        self.period = 0;
        self.enabled = false;
        self.length.power_off();
        self.envelope.power_off();
        if let Some(sweep) = self.sweep.as_mut() {
            sweep.power_off();
        }
        // The duty step resets with the power, which is why a freshly powered channel
        // always starts at the same point in its wave.
        self.step = 0;
    }
}

/// What a sweep tick decided.
///
/// Both fields can be set at once, and that combination is not a contradiction: the
/// hardware writes the new period back and *then* runs a second overflow check on it,
/// so a sweep step can update the note and kill the channel in the same breath.
#[derive(Default)]
struct SweepResult {
    /// A new period to write back to NR13/NR14.
    period: Option<u16>,
    /// The overflow check failed, so the channel must stop.
    overflowed: bool,
}

/// Channel 1's period sweep: slides the note up or down on its own.
///
/// It works on a private copy of the period called the shadow register, not on NR13/14
/// directly. That is why writing a new period while a sweep is running appears to work
/// and then gets stomped on the next sweep step — the shadow still holds the old value.
struct Sweep {
    /// NR10 as written, so reads give back exactly what was set.
    register: u8,
    shadow: u16,
    timer: u8,
    enabled: bool,
    /// Whether a subtracting calculation has happened since the last trigger. Only used
    /// for the quirk in [`Sweep::write`].
    negate_used: bool,
}

impl Sweep {
    fn new() -> Self {
        Sweep {
            register: 0,
            shadow: 0,
            timer: 0,
            enabled: false,
            negate_used: false,
        }
    }

    fn read(&self) -> u8 {
        self.register
    }

    fn pace(&self) -> u8 {
        (self.register >> 4) & 0x07
    }

    fn negate(&self) -> bool {
        self.register & 0x08 != 0
    }

    fn step(&self) -> u8 {
        self.register & 0x07
    }

    /// Returns true if the channel must be disabled.
    fn write(&mut self, value: u8) -> bool {
        let was_negating = self.negate();
        self.register = value;
        was_negating && !self.negate() && self.negate_used
    }

    /// Returns true if the immediate overflow check failed and the channel must stop.
    fn trigger(&mut self, period: u16) -> bool {
        self.shadow = period;
        self.reload_timer();
        // The unit runs if *either* field is non-zero: a pace with no step still ticks
        // (doing nothing), and a step with no pace still arms the overflow check.
        self.enabled = self.pace() != 0 || self.step() != 0;
        self.negate_used = false;

        // With a non-zero step the overflow check happens right away, before any timer
        // has elapsed — so a badly chosen sweep can kill the note instantly.
        self.step() != 0 && self.calculate().is_none()
    }

    fn tick(&mut self) -> SweepResult {
        if !self.enabled {
            return SweepResult::default();
        }

        // Saturating for the same reason as the envelope's: nothing reloads this
        // outside a trigger, so a write to NR10 can find it already at zero.
        self.timer = self.timer.saturating_sub(1);
        if self.timer != 0 {
            return SweepResult::default();
        }
        // The pace is re-read from the register only now, which is why a mid-note
        // change to NR10's pace does not take effect until the current step finishes.
        self.reload_timer();

        if self.pace() == 0 {
            return SweepResult::default();
        }

        let Some(new_period) = self.calculate() else {
            return SweepResult {
                period: None,
                overflowed: true,
            };
        };

        if self.step() == 0 {
            return SweepResult::default();
        }

        self.shadow = new_period;
        // A second calculation runs immediately, purely as an overflow check — its
        // result is discarded. This is why a sweep can stop one step earlier than the
        // arithmetic alone suggests: the note that would have played next is the one
        // whose *successor* overflows.
        SweepResult {
            period: Some(new_period),
            overflowed: self.calculate().is_none(),
        }
    }

    fn reload_timer(&mut self) {
        // A pace of 0 reloads as 8 rather than stalling, so the unit keeps ticking.
        self.timer = if self.pace() == 0 { 8 } else { self.pace() };
    }

    /// The next period, or `None` if it overflowed 11 bits.
    fn calculate(&mut self) -> Option<u16> {
        let delta = self.shadow >> self.step();
        let new_period = if self.negate() {
            self.negate_used = true;
            // Subtraction cannot underflow into the channel being disabled: the shift
            // makes delta no larger than the shadow itself.
            self.shadow.wrapping_sub(delta)
        } else {
            self.shadow + delta
        };

        (new_period <= 0x07FF).then_some(new_period)
    }

    fn power_off(&mut self) {
        self.register = 0;
        self.shadow = 0;
        self.timer = 0;
        self.enabled = false;
        self.negate_used = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel triggered at full volume with a mid-range period.
    fn playing(with_sweep: bool) -> Square {
        let mut square = Square::new(with_sweep);
        square.write(1, 0x80, false); // 50% duty
        square.write(2, 0xF0, false); // volume 15, no envelope
        square.write(3, 0x00, false);
        square.write(4, 0x84, false); // period 0x400, trigger
        square
    }

    #[test]
    fn the_duty_pattern_shapes_the_wave() {
        let mut square = playing(false);
        // 50% duty: four steps high out of eight, however they are arranged.
        let mut high = 0;
        for _ in 0..8 {
            if square.output() > 0 {
                high += 1;
            }
            for _ in 0..(2048 - 0x400) {
                square.tick();
            }
        }
        assert_eq!(high, 4);

        // 12.5% duty: one step in eight.
        square.write(1, 0x00, false);
        let mut high = 0;
        for _ in 0..8 {
            if square.output() > 0 {
                high += 1;
            }
            for _ in 0..(2048 - 0x400) {
                square.tick();
            }
        }
        assert_eq!(high, 1);
    }

    /// Counts how many duty steps the channel advances through in `ticks` M-cycles.
    fn steps_in(square: &mut Square, ticks: u32) -> u32 {
        let mut steps = 0;
        let mut previous = square.step;
        for _ in 0..ticks {
            square.tick();
            if square.step != previous {
                steps += 1;
                previous = square.step;
            }
        }
        steps
    }

    #[test]
    fn a_higher_period_value_is_a_faster_wave() {
        // The counter reloads to 2048 - period, so 0x700 (256 ticks per step) runs four
        // times as fast as 0x400 (1024) — two octaves up, even though the register
        // values are nowhere near a factor of four apart.
        let mut slow = playing(false);
        let mut fast = playing(false);
        fast.write(3, 0x00, false);
        fast.write(4, 0x87, false); // period 0x700

        const TICKS: u32 = 8 * 1024;
        assert_eq!(steps_in(&mut slow, TICKS), 8);
        assert_eq!(steps_in(&mut fast, TICKS), 32);
    }

    #[test]
    fn a_disabled_channel_outputs_digital_zero() {
        let mut square = playing(false);
        assert!(square.is_enabled());
        square.write(2, 0x00, false); // DAC off
        assert!(!square.is_enabled());
        assert_eq!(square.output(), 0);
    }

    #[test]
    fn a_trigger_cannot_start_a_channel_whose_dac_is_off() {
        let mut square = Square::new(false);
        square.write(2, 0x00, false);
        square.write(4, 0x80, false); // trigger
        assert!(!square.is_enabled());
    }

    #[test]
    fn the_length_timer_stops_the_channel() {
        let mut square = Square::new(false);
        square.write(2, 0xF0, false);
        square.write(1, 0x3F, false); // length 63, so one tick remains
        square.write(4, 0xC0, false); // trigger with length enabled
        assert!(square.is_enabled());
        square.tick_length();
        assert!(!square.is_enabled());
    }

    #[test]
    fn the_sweep_slides_the_period() {
        let mut square = Square::new(true);
        square.write(2, 0xF0, false);
        square.write(3, 0x00, false);
        square.write(4, 0x81, false); // period 0x100
        square.write(0, 0x13, false); // pace 1, adding, step 3
        square.write(4, 0x81, false); // re-trigger to load the shadow

        // 0x100 + 0x100>>3 = 0x120, and the follow-up check on 0x120 also fits.
        square.tick_sweep();
        assert_eq!(square.period, 0x120);
        assert!(square.is_enabled());

        square.tick_sweep();
        assert_eq!(square.period, 0x144);
    }

    #[test]
    fn the_step_that_overflows_still_writes_its_period_back() {
        // The hardware writes the new period, then runs a *second* overflow check on
        // it and stops the channel if that fails. So the last period a sweep produces
        // does land in the register even though nothing gets to play it.
        let mut square = Square::new(true);
        square.write(2, 0xF0, false);
        square.write(3, 0x00, false);
        square.write(4, 0x84, false); // period 0x400
        square.write(0, 0x11, false); // pace 1, adding, step 1
        square.write(4, 0x84, false);

        // 0x400 + 0x200 = 0x600 is written back; the check on 0x600 + 0x300 = 0x900
        // then overflows and kills the channel in the same step.
        square.tick_sweep();
        assert_eq!(square.period, 0x600, "the write-back happened");
        assert!(!square.is_enabled(), "and the second check stopped it");
    }

    #[test]
    fn a_subtracting_sweep_lowers_the_period() {
        let mut square = Square::new(true);
        square.write(2, 0xF0, false);
        square.write(3, 0x00, false);
        square.write(4, 0x84, false);
        square.write(0, 0x19, false); // pace 1, subtracting, step 1
        square.write(4, 0x84, false);

        square.tick_sweep();
        assert_eq!(square.period, 0x200);
        // Subtraction cannot overflow, so the channel stays on however far it slides.
        square.tick_sweep();
        assert!(square.is_enabled());
    }

    #[test]
    fn leaving_subtract_mode_after_a_calculation_kills_the_channel() {
        let mut square = Square::new(true);
        square.write(2, 0xF0, false);
        square.write(3, 0x00, false);
        square.write(4, 0x84, false);
        square.write(0, 0x19, false); // subtracting
        square.write(4, 0x84, false); // trigger: the immediate check uses subtraction
        assert!(square.is_enabled());

        // Switching back to addition now is the one thing the hardware refuses.
        square.write(0, 0x11, false);
        assert!(!square.is_enabled());
    }

    #[test]
    fn an_immediately_overflowing_sweep_never_plays() {
        let mut square = Square::new(true);
        square.write(2, 0xF0, false);
        square.write(3, 0xFF, false);
        square.write(4, 0x87, false); // period 0x7FF, as high as it goes
        square.write(0, 0x01, false); // pace 0, adding, step 1
        square.write(4, 0x87, false); // trigger runs the overflow check right away
        assert!(!square.is_enabled(), "0x7FF + 0x3FF cannot fit");
    }

    #[test]
    fn channel_two_has_no_sweep_register() {
        let mut square = Square::new(false);
        // NR20 does not exist; it reads as all ones and writes go nowhere.
        assert_eq!(square.read(0), 0xFF);
        square.write(0, 0x11, false);
        square.write(2, 0xF0, false);
        square.write(4, 0x84, false);
        square.tick_sweep();
        assert_eq!(square.period, 0x400, "no sweep unit to move it");
    }
}
