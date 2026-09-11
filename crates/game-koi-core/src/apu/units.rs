//! The two counters that several channels share: the length timer and the envelope.
//!
//! Both are clocked by the frame sequencer rather than by the channel's own frequency
//! timer, which is why they live here instead of inside a channel — a channel's pitch
//! has nothing to do with how fast it fades out.

/// Shuts a channel off after a set time.
///
/// The hardware counts *up* from the value the game writes until it hits 64 (or 256 for
/// the wave channel), which is why a *larger* written value means a *shorter* sound.
/// Counting down from `max - value` to zero is the same thing and makes the expiry test
/// obvious, so that is what this does — but it means [`LengthTimer::set`] takes the
/// value as the game wrote it and does the subtraction here, in one place.
pub(super) struct LengthTimer {
    counter: u16,
    /// 64 for the pulse and noise channels, 256 for the wave channel, whose length
    /// register is a full byte rather than six bits.
    max: u16,
    /// NRx4 bit 6. When clear the timer still counts, it just never shuts anything off.
    enabled: bool,
}

impl LengthTimer {
    pub(super) fn new(max: u16) -> Self {
        LengthTimer {
            counter: 0,
            max,
            enabled: false,
        }
    }

    pub(super) fn set(&mut self, value: u16) {
        self.counter = self.max - value;
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// On trigger, a timer that has run out is reloaded to full. One that has time left
    /// is untouched — retriggering does not extend a note that is still playing.
    ///
    /// `extra_clock` is the sequencer-phase quirk: a reload that lands in the half of
    /// the 512 Hz period that does not clock length comes up one short. See
    /// [`LengthTimer::extra_clock`].
    pub(super) fn trigger(&mut self, extra_clock: bool) {
        if self.counter == 0 {
            self.counter = self.max;
            if extra_clock && self.enabled {
                self.counter -= 1;
            }
        }
    }

    /// The out-of-band tick a write to NRx4 can cause.
    ///
    /// The length counter is clocked on the sequencer's even steps. Enabling it during
    /// the *odd* half — when the next step will not clock it — makes the hardware clock
    /// it once immediately instead, so the note ends where it would have anyway rather
    /// than a full step late. Returns true if that tick ran the timer out.
    ///
    /// This is not a curiosity: Blargg's `03-trigger` fails without it, and a game
    /// setting up a short note at the wrong moment gets an audibly wrong duration.
    pub(super) fn extra_clock(&mut self) -> bool {
        if self.counter == 0 {
            return false;
        }
        self.counter -= 1;
        self.counter == 0
    }

    /// Clocked at 256 Hz. Returns true on the tick that runs the timer out, which is
    /// the channel's cue to shut itself off.
    pub(super) fn tick(&mut self) -> bool {
        if !self.enabled || self.counter == 0 {
            return false;
        }
        self.counter -= 1;
        self.counter == 0
    }

    pub(super) fn power_off(&mut self) {
        self.enabled = false;
    }
}

/// Fades a channel's volume up or down over time.
///
/// The envelope changes the volume the channel *outputs*; it never touches NRx2, which
/// is why reading that register back gives you the initial volume however long the note
/// has been fading. It is also why the envelope reaching zero does not turn the channel
/// off — the DAC is still on, still converting a digital zero.
pub(super) struct Envelope {
    /// NRx2 as written. Kept whole because the DAC-enable test looks at the raw bits.
    register: u8,
    /// The volume actually in use, 0-15.
    volume: u8,
    timer: u8,
}

impl Envelope {
    pub(super) fn new() -> Self {
        Envelope {
            register: 0,
            volume: 0,
            timer: 0,
        }
    }

    pub(super) fn read(&self) -> u8 {
        self.register
    }

    pub(super) fn write(&mut self, value: u8) {
        self.register = value;
    }

    /// Whether the channel's DAC is powered.
    ///
    /// Any of the top five bits set is enough: those are the initial volume and the
    /// direction. Writing 0x00 (volume 0, decreasing) is the documented way to cut the
    /// DAC — and it takes the channel with it, since a channel cannot be on with its
    /// DAC off.
    pub(super) fn dac_enabled(&self) -> bool {
        self.register & 0xF8 != 0
    }

    fn pace(&self) -> u8 {
        self.register & 0x07
    }

    fn increasing(&self) -> bool {
        self.register & 0x08 != 0
    }

    pub(super) fn volume(&self) -> u8 {
        self.volume
    }

    pub(super) fn trigger(&mut self) {
        self.volume = self.register >> 4;
        self.timer = self.pace();
    }

    /// Clocked at 64 Hz.
    pub(super) fn tick(&mut self) {
        // A pace of zero switches the envelope off entirely rather than meaning "every
        // tick" — the note holds whatever volume it has.
        if self.pace() == 0 {
            return;
        }

        // Saturating, not plain subtraction: the timer is only loaded by a trigger, so
        // a game that writes a non-zero pace into NRx2 without retriggering leaves it
        // at zero here. On hardware that just means the next tick fires immediately.
        self.timer = self.timer.saturating_sub(1);
        if self.timer != 0 {
            return;
        }
        self.timer = self.pace();

        // The volume sticks at the ends of its range instead of wrapping, so a fade-out
        // stays faded out rather than jumping back to full.
        if self.increasing() && self.volume < 15 {
            self.volume += 1;
        } else if !self.increasing() && self.volume > 0 {
            self.volume -= 1;
        }
    }

    pub(super) fn power_off(&mut self) {
        self.register = 0;
        self.volume = 0;
        self.timer = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bigger_written_length_means_a_shorter_sound() {
        let mut short = LengthTimer::new(64);
        short.set_enabled(true);
        short.set(63);

        let mut long = LengthTimer::new(64);
        long.set_enabled(true);
        long.set(0);

        assert!(short.tick(), "length 63 leaves one tick");
        assert!(!long.tick(), "length 0 leaves the full 64");
    }

    #[test]
    fn a_disabled_timer_never_expires() {
        let mut length = LengthTimer::new(64);
        length.set(63);
        for _ in 0..100 {
            assert!(!length.tick());
        }
    }

    #[test]
    fn an_extra_clock_shortens_a_reload_by_one() {
        // Triggering during the half of the sequencer period that does not clock length
        // loads 63 rather than 64, so the note ends when it would have anyway.
        let mut length = LengthTimer::new(64);
        length.set_enabled(true);
        length.trigger(true);
        for _ in 0..62 {
            assert!(!length.tick());
        }
        assert!(length.tick(), "63 ticks, not 64");
    }

    #[test]
    fn an_extra_clock_only_applies_to_an_enabled_timer() {
        let mut length = LengthTimer::new(64);
        length.trigger(true);
        length.set_enabled(true);
        for _ in 0..63 {
            assert!(!length.tick());
        }
        assert!(length.tick(), "the full 64");
    }

    #[test]
    fn trigger_reloads_only_an_expired_timer() {
        let mut length = LengthTimer::new(64);
        length.set_enabled(true);
        length.set(62);

        // Two ticks left; triggering now must not extend it to 64.
        length.trigger(false);
        assert!(!length.tick());
        assert!(length.tick(), "still expired on schedule");

        // Now it has run out, so a trigger reloads it to full.
        length.trigger(false);
        for _ in 0..63 {
            assert!(!length.tick());
        }
        assert!(length.tick());
    }

    #[test]
    fn the_envelope_fades_at_its_pace() {
        let mut envelope = Envelope::new();
        envelope.write(0xF2); // volume 15, decreasing, pace 2
        envelope.trigger();
        assert_eq!(envelope.volume(), 15);

        envelope.tick();
        assert_eq!(envelope.volume(), 15, "pace 2 needs two ticks");
        envelope.tick();
        assert_eq!(envelope.volume(), 14);
    }

    #[test]
    fn a_pace_of_zero_holds_the_volume() {
        let mut envelope = Envelope::new();
        envelope.write(0xF0); // volume 15, decreasing, pace 0
        envelope.trigger();
        for _ in 0..100 {
            envelope.tick();
        }
        assert_eq!(envelope.volume(), 15);
    }

    #[test]
    fn raising_the_pace_without_a_trigger_does_not_underflow() {
        // A game that starts with a pace of 0 and later writes a non-zero one, without
        // retriggering, leaves the timer at 0. Blargg's `01-registers` does exactly
        // this; before the timer saturated, a debug build panicked here.
        let mut envelope = Envelope::new();
        envelope.write(0xF0); // volume 15, pace 0
        envelope.trigger();
        envelope.write(0xF3); // pace 3, no trigger
        for _ in 0..10 {
            envelope.tick();
        }
        assert!(envelope.volume() < 15, "the envelope started moving");
    }

    #[test]
    fn the_volume_sticks_at_the_ends() {
        let mut envelope = Envelope::new();
        envelope.write(0x11); // volume 1, decreasing, pace 1
        envelope.trigger();
        envelope.tick();
        assert_eq!(envelope.volume(), 0);
        envelope.tick();
        assert_eq!(envelope.volume(), 0, "does not wrap back to 15");
    }

    #[test]
    fn the_envelope_does_not_rewrite_its_register() {
        // Reading NRx2 back gives the initial volume, not the faded one — this is why
        // a game cannot use NRx2 to find out how loud a channel currently is.
        let mut envelope = Envelope::new();
        envelope.write(0xF1);
        envelope.trigger();
        envelope.tick();
        assert_eq!(envelope.volume(), 14);
        assert_eq!(envelope.read(), 0xF1);
    }

    #[test]
    fn the_dac_needs_one_of_the_top_five_bits() {
        let mut envelope = Envelope::new();
        envelope.write(0x00);
        assert!(!envelope.dac_enabled());
        // Volume 0 but increasing: still powered.
        envelope.write(0x08);
        assert!(envelope.dac_enabled());
        // A pace alone is not enough.
        envelope.write(0x07);
        assert!(!envelope.dac_enabled());
    }
}
