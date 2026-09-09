//! The timer: DIV, TIMA, TMA, and TAC.
//!
//! # The one idea that explains everything
//!
//! There is a single 16-bit counter that increments once per T-cycle and is never
//! reset except by a write to DIV. Everything else is a view onto it:
//!
//! - **DIV** (0xFF04) is not its own counter — it is the *high byte* of that 16-bit
//!   value. The register appendix names it `DIVH<7:0>`, which is the giveaway. Since
//!   the low byte wraps every 256 T-cycles, DIV appears to tick at 16384 Hz.
//! - **TIMA** (0xFF05) increments on a **falling edge** of one selected bit of the
//!   counter, ANDed with the timer-enable bit of TAC.
//!
//! Falling-edge detection rather than "every N cycles" is the whole game. It means
//! the timer's phase is tied to the shared counter, not to when the timer was
//! switched on, and it is why the quirks below exist at all — they are not special
//! cases in the hardware, just consequences of watching one bit fall.
//!
//! # The quirks that fall out
//!
//! - Writing **any** value to DIV resets the whole 16-bit counter to zero. If the
//!   watched bit was 1 at that moment, zeroing it *is* a falling edge, so TIMA
//!   increments as a side effect of a DIV write.
//! - Disabling the timer in TAC while the watched bit is 1 likewise produces a
//!   falling edge, ticking TIMA on the way down.
//! - Changing the TAC frequency can tick TIMA, if the old bit was 1 and the new one
//!   is 0.
//!
//! # The overflow delay
//!
//! When TIMA overflows past 0xFF it does **not** reload from TMA immediately. For
//! one M-cycle it reads as 0x00, and only then is TMA loaded and the interrupt
//! requested. Writes landing in that window behave specially, as noted at the write
//! sites below. Mooneye's timer tests check all of this precisely.

use crate::interrupts::{Interrupt, InterruptState};

/// State of the pending TIMA overflow, which takes effect one M-cycle late.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overflow {
    /// Not overflowing.
    None,
    /// TIMA overflowed this M-cycle: it reads 0x00 and the reload is due next cycle.
    Pending,
    /// The reload happened this M-cycle. Tracked because a write to TIMA during the
    /// reload cycle is ignored, unlike one during the pending cycle.
    Reloaded,
}

#[derive(Debug, Clone)]
pub struct Timer {
    /// The 16-bit system counter. DIV is its high byte.
    counter: u16,
    /// TIMA (0xFF05) — the counter that actually raises interrupts.
    tima: u8,
    /// TMA (0xFF06) — the value TIMA reloads from after overflowing.
    tma: u8,
    /// TAC (0xFF07) — bit 2 enables the timer, bits 0-1 select the frequency.
    tac: u8,
    /// Last observed state of (selected bit AND enable), for edge detection.
    last_edge_bit: bool,
    overflow: Overflow,
}

impl Timer {
    /// Post-boot state.
    ///
    /// The counter is nonzero because the boot ROM has been running for a while
    /// before the cartridge starts; 0xABCC is what a DMG leaves behind (DIV reads
    /// 0xAB). Games that seed a random number generator from DIV depend on this.
    pub fn new() -> Self {
        Timer {
            counter: 0xABCC,
            tima: 0x00,
            tma: 0x00,
            tac: 0xF8,
            last_edge_bit: false,
            overflow: Overflow::None,
        }
    }

    /// Which bit of the 16-bit counter TAC selects.
    ///
    /// The four frequencies are 4096, 262144, 65536, and 16384 Hz. Note the order:
    /// slot 0 is the *slowest*, not the fastest, which is easy to get backwards.
    fn selected_bit(tac: u8) -> u8 {
        match tac & 0x03 {
            0 => 9, // 4096 Hz
            1 => 3, // 262144 Hz
            2 => 5, // 65536 Hz
            _ => 7, // 16384 Hz
        }
    }

    fn timer_enabled(&self) -> bool {
        self.tac & 0x04 != 0
    }

    /// The value the edge detector watches: the selected counter bit ANDed with the
    /// enable bit. TIMA increments whenever this goes from true to false.
    fn edge_bit(&self) -> bool {
        let bit = (self.counter >> Self::selected_bit(self.tac)) & 1 != 0;
        bit && self.timer_enabled()
    }

    /// Advances the timer by one M-cycle (4 T-cycles).
    pub fn tick(&mut self, interrupts: &mut InterruptState) {
        // Resolve a pending overflow from the previous M-cycle first: TIMA read as
        // 0x00 for exactly one cycle, and now the reload lands.
        match self.overflow {
            Overflow::Pending => {
                self.tima = self.tma;
                self.overflow = Overflow::Reloaded;
                interrupts.request(Interrupt::Timer);
            }
            Overflow::Reloaded => self.overflow = Overflow::None,
            Overflow::None => {}
        }

        // The counter runs at the T-cycle rate, so one M-cycle is four steps. They
        // have to be stepped individually because the fastest TAC setting watches
        // bit 3, which changes within a single M-cycle.
        for _ in 0..4 {
            self.counter = self.counter.wrapping_add(1);
            self.detect_edge();
        }
    }

    /// Increments TIMA if the watched bit just fell.
    fn detect_edge(&mut self) {
        let current = self.edge_bit();
        if self.last_edge_bit && !current {
            self.increment_tima();
        }
        self.last_edge_bit = current;
    }

    fn increment_tima(&mut self) {
        let (result, overflowed) = self.tima.overflowing_add(1);
        self.tima = result;
        if overflowed {
            // Leave TIMA at 0x00 and schedule the reload for the next M-cycle.
            self.overflow = Overflow::Pending;
        }
    }

    pub fn read(&self, address: u16) -> u8 {
        match address {
            0xFF04 => (self.counter >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            // Only 3 bits of TAC exist; the rest read as 1.
            0xFF07 => self.tac | 0xF8,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            // Any write resets the whole counter. If the watched bit was high, this
            // is a falling edge and TIMA increments — a real, observable side effect
            // of writing DIV, not an accident of this implementation.
            0xFF04 => {
                self.counter = 0;
                self.detect_edge();
            }
            // A write during the reload cycle is ignored: TMA has already won.
            // During the pending cycle, though, the write lands and cancels the
            // overflow, so no interrupt fires.
            0xFF05 if self.overflow != Overflow::Reloaded => {
                self.tima = value;
                self.overflow = Overflow::None;
            }
            0xFF05 => {}
            0xFF06 => {
                self.tma = value;
                // Writing TMA on the reload cycle also updates TIMA, since the
                // reload is wired straight through.
                if self.overflow == Overflow::Reloaded {
                    self.tima = value;
                }
            }
            // Changing frequency or disabling the timer can drop the watched bit
            // from 1 to 0, which the edge detector sees as a tick.
            0xFF07 => {
                self.tac = value & 0x07;
                self.detect_edge();
            }
            _ => {}
        }
    }

    /// DIV as the CPU sees it. Exposed for tests and tracing.
    pub fn div(&self) -> u8 {
        (self.counter >> 8) as u8
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timer with a zeroed counter, so tests can count cycles from a known point.
    fn fresh(tac: u8) -> (Timer, InterruptState) {
        let mut timer = Timer::new();
        timer.counter = 0;
        timer.last_edge_bit = false;
        timer.tac = tac;
        let interrupts = InterruptState {
            enabled: 0xFF,
            ..Default::default()
        };
        (timer, interrupts)
    }

    fn tick_n(timer: &mut Timer, interrupts: &mut InterruptState, n: usize) {
        for _ in 0..n {
            timer.tick(interrupts);
        }
    }

    #[test]
    fn div_is_the_high_byte_of_the_counter() {
        let (mut timer, mut ints) = fresh(0x00);
        // 256 T-cycles = 64 M-cycles per DIV increment.
        tick_n(&mut timer, &mut ints, 63);
        assert_eq!(timer.div(), 0);
        tick_n(&mut timer, &mut ints, 1);
        assert_eq!(timer.div(), 1);
    }

    #[test]
    fn div_runs_even_when_the_timer_is_disabled() {
        // TAC bit 2 clear: TIMA is frozen but DIV keeps counting.
        let (mut timer, mut ints) = fresh(0x00);
        tick_n(&mut timer, &mut ints, 64);
        assert_eq!(timer.div(), 1);
        assert_eq!(timer.tima, 0, "TIMA must not run while disabled");
    }

    #[test]
    fn writing_div_resets_the_whole_counter() {
        let (mut timer, mut ints) = fresh(0x00);
        tick_n(&mut timer, &mut ints, 100);
        assert_ne!(timer.div(), 0);
        timer.write(0xFF04, 0xFF); // The value written is irrelevant.
        assert_eq!(timer.div(), 0);
        assert_eq!(timer.counter, 0);
    }

    #[test]
    fn tima_increments_at_the_selected_frequency() {
        // TAC = 0b101: enabled, slot 1 -> bit 3 -> every 16 T-cycles = 4 M-cycles.
        let (mut timer, mut ints) = fresh(0x05);
        tick_n(&mut timer, &mut ints, 4);
        assert_eq!(timer.tima, 1);
        tick_n(&mut timer, &mut ints, 4);
        assert_eq!(timer.tima, 2);

        // TAC = 0b100: slot 0 -> bit 9 -> every 1024 T-cycles = 256 M-cycles.
        // Slot 0 is the slowest setting, not the fastest.
        let (mut timer, mut ints) = fresh(0x04);
        tick_n(&mut timer, &mut ints, 255);
        assert_eq!(timer.tima, 0);
        tick_n(&mut timer, &mut ints, 1);
        assert_eq!(timer.tima, 1);
    }

    #[test]
    fn all_four_frequencies_are_distinct() {
        // M-cycles per TIMA tick for TAC slots 0..3.
        for (slot, expected) in [(0, 256), (1, 4), (2, 16), (3, 64)] {
            let (mut timer, mut ints) = fresh(0x04 | slot);
            tick_n(&mut timer, &mut ints, expected - 1);
            assert_eq!(timer.tima, 0, "slot {slot} ticked early");
            tick_n(&mut timer, &mut ints, 1);
            assert_eq!(timer.tima, 1, "slot {slot} did not tick at {expected}");
        }
    }

    #[test]
    fn overflow_reloads_from_tma_and_requests_an_interrupt() {
        let (mut timer, mut ints) = fresh(0x05);
        timer.tma = 0xAB;
        timer.tima = 0xFF;

        // The tick that overflows: TIMA reads 0x00 and no interrupt yet.
        tick_n(&mut timer, &mut ints, 4);
        assert_eq!(timer.tima, 0x00, "TIMA reads zero for one cycle");
        assert!(!ints.any_pending(), "interrupt is delayed by one M-cycle");

        // The next cycle performs the reload and raises the interrupt.
        tick_n(&mut timer, &mut ints, 1);
        assert_eq!(timer.tima, 0xAB);
        assert_eq!(ints.pending(), Some(Interrupt::Timer));
    }

    #[test]
    fn writing_tima_during_the_pending_cycle_cancels_the_overflow() {
        let (mut timer, mut ints) = fresh(0x05);
        timer.tma = 0xAB;
        timer.tima = 0xFF;
        tick_n(&mut timer, &mut ints, 4); // Overflow is now pending.

        timer.write(0xFF05, 0x42);
        tick_n(&mut timer, &mut ints, 1);
        assert_eq!(timer.tima, 0x42, "the write beat the reload");
        assert!(
            !ints.any_pending(),
            "cancelled overflow raises no interrupt"
        );
    }

    #[test]
    fn writing_tima_during_the_reload_cycle_is_ignored() {
        let (mut timer, mut ints) = fresh(0x05);
        timer.tma = 0xAB;
        timer.tima = 0xFF;
        tick_n(&mut timer, &mut ints, 5); // Through the reload.
        assert_eq!(timer.tima, 0xAB);

        timer.write(0xFF05, 0x42);
        assert_eq!(timer.tima, 0xAB, "TMA already won this cycle");
    }

    #[test]
    fn writing_tma_during_the_reload_cycle_also_sets_tima() {
        let (mut timer, mut ints) = fresh(0x05);
        timer.tma = 0xAB;
        timer.tima = 0xFF;
        tick_n(&mut timer, &mut ints, 5);

        timer.write(0xFF06, 0x99);
        assert_eq!(timer.tima, 0x99, "the reload is wired straight through");
    }

    #[test]
    fn writing_div_can_tick_tima() {
        // The famous quirk. With bit 3 selected, run until it is high, then reset
        // the counter: that 1 -> 0 transition is a falling edge.
        let (mut timer, mut ints) = fresh(0x05);
        tick_n(&mut timer, &mut ints, 2); // counter = 8, so bit 3 is set.
        assert_eq!(timer.tima, 0);

        timer.write(0xFF04, 0x00);
        assert_eq!(timer.tima, 1, "resetting DIV produced a falling edge");
    }

    #[test]
    fn disabling_the_timer_can_tick_tima() {
        let (mut timer, mut ints) = fresh(0x05);
        tick_n(&mut timer, &mut ints, 2); // Bit 3 high.
        assert_eq!(timer.tima, 0);

        timer.write(0xFF07, 0x01); // Clear the enable bit.
        assert_eq!(timer.tima, 1, "disabling dropped the ANDed bit to zero");
    }

    #[test]
    fn changing_frequency_can_tick_tima() {
        // Start on bit 3 with it high, switch to bit 5 while that one is low.
        let (mut timer, mut ints) = fresh(0x05);
        tick_n(&mut timer, &mut ints, 2); // counter = 8: bit 3 set, bit 5 clear.
        assert_eq!(timer.tima, 0);

        timer.write(0xFF07, 0x06); // Enabled, slot 2 -> bit 5.
        assert_eq!(timer.tima, 1, "old bit fell, new bit was already low");
    }

    #[test]
    fn tac_and_if_read_back_with_unused_bits_set() {
        let (mut timer, _) = fresh(0x00);
        timer.write(0xFF07, 0x05);
        assert_eq!(timer.read(0xFF07), 0xFD, "top 5 bits read as 1");
    }

    #[test]
    fn timer_is_periodic_over_many_overflows() {
        // A sanity check on the whole pipeline: with TMA = 0xFE, TIMA overflows
        // every 2 ticks of the selected bit. Over a long run the interrupt count
        // should track that rate.
        let (mut timer, mut ints) = fresh(0x05); // bit 3: every 4 M-cycles
        timer.tma = 0xFE;
        timer.tima = 0xFE;

        let mut interrupts_seen = 0;
        for _ in 0..1000 {
            timer.tick(&mut ints);
            if ints.any_pending() {
                interrupts_seen += 1;
                ints.acknowledge(Interrupt::Timer);
            }
        }
        // Naively 1000 / (4 per increment * 2 increments) = 125, but each overflow
        // also spends an M-cycle in the reload delay before the interrupt is
        // raised, so the last one lands just past the end of the run.
        assert_eq!(interrupts_seen, 124);
    }
}
