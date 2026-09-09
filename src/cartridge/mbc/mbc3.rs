//! MBC3: up to 2 MiB of ROM, 32 KiB of RAM, and a clock that keeps time with the
//! console switched off.
//!
//! The banking is the boring part, and deliberately so — MBC3 drops MBC1's mode bit and
//! gives the ROM bank register a full 7 bits, so `BANK` means what it says and the
//! 0x20/0x40/0x60 gap is gone. What it adds is the RTC.
//!
//! # The clock
//!
//! A quartz crystal and a battery on the cartridge keep five counters running whether or
//! not the Game Boy is on: seconds, minutes, hours, a 9-bit day counter, and a carry bit
//! for when that day counter wraps after 512 days. Pokémon Gold's berry trees and Animal
//! Crossing-style "come back tomorrow" mechanics are what this is for.
//!
//! The counters are not in the memory map. They appear in the 0xA000-0xBFFF window
//! *instead of* save RAM when the bank register selects 0x08-0x0C, which is why the same
//! register does double duty.
//!
//! # Latching
//!
//! Reading five counters that are still counting would let you observe 00:59:59 in the
//! middle of becoming 01:00:00 — minutes read before the rollover, hours after. So the
//! CPU cannot read them live. Writing 0x00 then 0x01 to 0x6000-0x7FFF copies all five
//! into a set of latch registers at one instant, and those are what reads return. The
//! clock behind them never stops.
//!
//! We drive it from the emulator's own cycle count rather than the host clock: the
//! cartridge is on the same crystal as everything else here, so a paused emulator has a
//! paused cartridge, and a run is reproducible.

use super::{Mbc, OPEN_BUS, Ram, Rom};

/// M-cycles in one second: 4194304 T-cycles / 4.
const M_CYCLES_PER_SECOND: u32 = 1_048_576;

/// The day counter is 9 bits, so it wraps after this many days and sets the carry.
const DAYS_PER_WRAP: u16 = 512;

/// The five clock counters, as a value that can be copied wholesale on a latch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Time {
    seconds: u8,
    minutes: u8,
    hours: u8,
    /// 0-511. The high bit lives in the DH register alongside the flags.
    days: u16,
    /// Set when the day counter wraps past 511, and never cleared by the hardware —
    /// only by the game writing DH. It is how a game notices it has been more than 512
    /// days, which it otherwise could not distinguish from a fresh cartridge.
    carry: bool,
}

impl Time {
    /// Advances by one second, rolling each counter into the next.
    fn advance(&mut self) {
        self.seconds += 1;
        if self.seconds < 60 {
            return;
        }
        self.seconds = 0;

        self.minutes += 1;
        if self.minutes < 60 {
            return;
        }
        self.minutes = 0;

        self.hours += 1;
        if self.hours < 24 {
            return;
        }
        self.hours = 0;

        self.days += 1;
        if self.days >= DAYS_PER_WRAP {
            self.days = 0;
            self.carry = true;
        }
    }
}

/// The clock: live counters, the latched copy the CPU reads, and the halt bit.
struct Rtc {
    live: Time,
    latched: Time,
    /// DH bit 6. While set the crystal is disconnected and nothing advances — games
    /// set it before writing the counters so the write is not immediately overtaken.
    halted: bool,
    /// M-cycles accumulated toward the next whole second.
    cycles: u32,
    /// Whether the 0x00 half of the 0x00-then-0x01 latch sequence has been seen.
    latch_armed: bool,
}

impl Rtc {
    fn new() -> Self {
        Rtc {
            live: Time::default(),
            latched: Time::default(),
            halted: false,
            cycles: 0,
            latch_armed: false,
        }
    }

    fn tick(&mut self) {
        if self.halted {
            return;
        }
        self.cycles += 1;
        if self.cycles >= M_CYCLES_PER_SECOND {
            self.cycles -= M_CYCLES_PER_SECOND;
            self.live.advance();
        }
    }

    /// Handles a write to 0x6000-0x7FFF. The latch happens on the 0->1 edge, so a game
    /// that writes 0x01 twice does not latch twice.
    fn write_latch(&mut self, value: u8) {
        match value {
            0x00 => self.latch_armed = true,
            0x01 if self.latch_armed => {
                self.latched = self.live;
                self.latch_armed = false;
            }
            // Anything else abandons the sequence rather than completing it.
            _ => self.latch_armed = false,
        }
    }

    /// Reads one of the RTC registers, selected by 0x08-0x0C.
    fn read(&self, register: u8) -> u8 {
        match register {
            0x08 => self.latched.seconds,
            0x09 => self.latched.minutes,
            0x0A => self.latched.hours,
            0x0B => self.latched.days as u8,
            0x0C => {
                // DH packs the day counter's 9th bit in with the two flags; the
                // unwired bits in between read high.
                let day_high = (self.latched.days >> 8) as u8 & 0x01;
                let halt = if self.halted { 0x40 } else { 0 };
                let carry = if self.latched.carry { 0x80 } else { 0 };
                day_high | halt | carry | 0x3E
            }
            _ => OPEN_BUS,
        }
    }

    /// Writes one of the RTC registers.
    ///
    /// The write lands on both the live counters and the latch: the game is setting the
    /// clock, so leaving the latch showing the old time until the next latch sequence
    /// would just be confusing. Writing seconds also clears the sub-second divider, as
    /// on hardware — which is how a game sets the clock to an exact second.
    fn write(&mut self, register: u8, value: u8) {
        match register {
            0x08 => {
                self.live.seconds = value % 60;
                self.cycles = 0;
            }
            0x09 => self.live.minutes = value % 60,
            0x0A => self.live.hours = value % 24,
            0x0B => self.live.days = (self.live.days & 0x0100) | value as u16,
            0x0C => {
                self.live.days = (self.live.days & 0x00FF) | ((value as u16 & 0x01) << 8);
                self.halted = value & 0x40 != 0;
                self.live.carry = value & 0x80 != 0;
            }
            _ => return,
        }
        self.latched = self.live;
    }
}

/// What the 0xA000-0xBFFF window is currently showing.
enum Window {
    Ram(usize),
    Clock(u8),
}

pub(super) struct Mbc3 {
    rom: Rom,
    ram: Ram,
    /// 7-bit ROM bank for the high window. Zero-adjusted, but with no BANK2 to prepend
    /// there is no gap: only bank 0 itself is unreachable there.
    bank: u8,
    /// 0x00-0x07 selects a RAM bank, 0x08-0x0C an RTC register.
    select: u8,
    /// `None` on a cartridge with no crystal — the +RAM variants of the type byte.
    rtc: Option<Rtc>,
}

impl Mbc3 {
    pub(super) fn new(rom: Vec<u8>, rom_banks: usize, ram_size: usize, has_timer: bool) -> Self {
        Mbc3 {
            rom: Rom::new(rom, rom_banks),
            ram: Ram::new(ram_size),
            bank: 1,
            select: 0,
            rtc: has_timer.then(Rtc::new),
        }
    }

    fn window(&self) -> Window {
        match self.select {
            // 0x00-0x07 rather than 0x00-0x03: MBC30, the variant in Japanese Pokemon
            // Crystal, has eight RAM banks. On a plain MBC3 the extra numbers simply
            // wrap in the RAM chip, which is what the missing address lines would do.
            0x00..=0x07 => Window::Ram(self.select as usize),
            register => Window::Clock(register),
        }
    }
}

impl Mbc for Mbc3 {
    fn ram(&self) -> Option<&Ram> {
        Some(&self.ram)
    }

    fn ram_mut(&mut self) -> Option<&mut Ram> {
        Some(&mut self.ram)
    }

    fn read_rom(&self, address: u16) -> u8 {
        let bank = if address < 0x4000 {
            0
        } else {
            self.bank as usize
        };
        self.rom.read(bank, address)
    }

    fn write_rom(&mut self, address: u16, value: u8) {
        match address {
            // One gate for both the RAM and the clock registers: with the crystal
            // running off its own battery, this only controls access, not timekeeping.
            0x0000..=0x1FFF => self.ram.set_enabled(value & 0x0F == 0x0A),

            // Seven bits, so 2 MiB is reachable without a second register. Still
            // zero-adjusted, for MBC1's reason: bank 0 is already in the low window.
            0x2000..=0x3FFF => {
                let bank = value & 0x7F;
                self.bank = if bank == 0 { 1 } else { bank };
            }

            0x4000..=0x5FFF => self.select = value,

            0x6000..=0x7FFF => {
                if let Some(rtc) = self.rtc.as_mut() {
                    rtc.write_latch(value);
                }
            }

            _ => unreachable!("write_rom called outside 0x0000-0x7FFF"),
        }
    }

    fn read_ram(&self, address: u16) -> u8 {
        match self.window() {
            Window::Ram(bank) => self.ram.read(bank, address),
            Window::Clock(register) => match self.rtc.as_ref() {
                // The gate covers the clock too: a game must open it before reading the
                // time, exactly as for save RAM.
                Some(rtc) if self.ram.enabled => rtc.read(register),
                _ => OPEN_BUS,
            },
        }
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        match self.window() {
            Window::Ram(bank) => self.ram.write(bank, address, value),
            Window::Clock(register) => {
                if self.ram.enabled
                    && let Some(rtc) = self.rtc.as_mut()
                {
                    rtc.write(register, value);
                }
            }
        }
    }

    fn tick(&mut self) {
        if let Some(rtc) = self.rtc.as_mut() {
            rtc.tick();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::mbc::banked_rom;

    fn mbc3(banks: usize, ram_bytes: usize) -> Mbc3 {
        let mut mbc = Mbc3::new(banked_rom(banks), banks, ram_bytes, true);
        // Open the gate; almost every test needs it.
        mbc.write_rom(0x0000, 0x0A);
        mbc
    }

    fn tick_seconds(mbc: &mut Mbc3, seconds: u32) {
        for _ in 0..seconds * M_CYCLES_PER_SECOND {
            mbc.tick();
        }
    }

    /// Latches the clock and reads one register, which is the only way a game can.
    fn read_clock(mbc: &mut Mbc3, register: u8) -> u8 {
        mbc.write_rom(0x6000, 0x00);
        mbc.write_rom(0x6000, 0x01);
        mbc.write_rom(0x4000, register);
        mbc.read_ram(0xA000)
    }

    #[test]
    fn seven_bank_bits_and_no_gap() {
        let mut mbc = mbc3(128, 0);
        // 0x20 is a perfectly ordinary bank here, unlike on MBC1.
        mbc.write_rom(0x2000, 0x20);
        assert_eq!(mbc.read_rom(0x4000), 0x20);
        mbc.write_rom(0x2000, 0x7F);
        assert_eq!(mbc.read_rom(0x4000), 0x7F);
    }

    #[test]
    fn bank_zero_becomes_bank_one() {
        let mut mbc = mbc3(128, 0);
        mbc.write_rom(0x2000, 0);
        assert_eq!(mbc.read_rom(0x4000), 1);
        assert_eq!(mbc.read_rom(0x0000), 0);
    }

    #[test]
    fn ram_banks_switch_without_a_mode_bit() {
        let mut mbc = mbc3(16, 32 * 1024);
        for bank in 0..4u8 {
            mbc.write_rom(0x4000, bank);
            mbc.write_ram(0xA000, 0xC0 | bank);
        }
        for bank in 0..4u8 {
            mbc.write_rom(0x4000, bank);
            assert_eq!(mbc.read_ram(0xA000), 0xC0 | bank);
        }
    }

    #[test]
    fn the_clock_replaces_ram_in_the_window() {
        let mut mbc = mbc3(16, 32 * 1024);
        mbc.write_rom(0x4000, 0);
        mbc.write_ram(0xA000, 0x42);

        // Selecting 0x08 swaps the clock in; the RAM byte is hidden, not lost.
        tick_seconds(&mut mbc, 5);
        assert_eq!(read_clock(&mut mbc, 0x08), 5);

        mbc.write_rom(0x4000, 0);
        assert_eq!(mbc.read_ram(0xA000), 0x42);
    }

    #[test]
    fn the_clock_counts_seconds() {
        let mut mbc = mbc3(16, 0);
        tick_seconds(&mut mbc, 59);
        assert_eq!(read_clock(&mut mbc, 0x08), 59);
        assert_eq!(read_clock(&mut mbc, 0x09), 0);

        // One more second rolls into minutes.
        tick_seconds(&mut mbc, 1);
        assert_eq!(read_clock(&mut mbc, 0x08), 0);
        assert_eq!(read_clock(&mut mbc, 0x09), 1);
    }

    #[test]
    fn reads_show_the_latched_time_not_the_live_one() {
        let mut mbc = mbc3(16, 0);
        tick_seconds(&mut mbc, 3);

        // Latch, then let time pass without latching again.
        mbc.write_rom(0x6000, 0x00);
        mbc.write_rom(0x6000, 0x01);
        mbc.write_rom(0x4000, 0x08);
        tick_seconds(&mut mbc, 10);
        assert_eq!(mbc.read_ram(0xA000), 3, "the latch is frozen");

        // The clock behind it kept running the whole time.
        assert_eq!(read_clock(&mut mbc, 0x08), 13);
    }

    #[test]
    fn latching_needs_the_full_zero_then_one_sequence() {
        let mut mbc = mbc3(16, 0);
        mbc.write_rom(0x4000, 0x08);
        tick_seconds(&mut mbc, 4);

        // A lone 0x01 does not latch.
        mbc.write_rom(0x6000, 0x01);
        assert_eq!(mbc.read_ram(0xA000), 0);

        // Nor does a sequence interrupted by another value.
        mbc.write_rom(0x6000, 0x00);
        mbc.write_rom(0x6000, 0xFF);
        mbc.write_rom(0x6000, 0x01);
        assert_eq!(mbc.read_ram(0xA000), 0);

        mbc.write_rom(0x6000, 0x00);
        mbc.write_rom(0x6000, 0x01);
        assert_eq!(mbc.read_ram(0xA000), 4);
    }

    #[test]
    fn halting_stops_the_clock() {
        let mut mbc = mbc3(16, 0);
        mbc.write_rom(0x4000, 0x0C);
        mbc.write_ram(0xA000, 0x40); // DH bit 6: halt
        tick_seconds(&mut mbc, 30);
        assert_eq!(read_clock(&mut mbc, 0x08), 0, "a halted clock does not run");

        // Clearing it starts the crystal again.
        mbc.write_rom(0x4000, 0x0C);
        mbc.write_ram(0xA000, 0x00);
        tick_seconds(&mut mbc, 7);
        assert_eq!(read_clock(&mut mbc, 0x08), 7);
    }

    #[test]
    fn writing_the_clock_sets_it() {
        let mut mbc = mbc3(16, 0);
        mbc.write_rom(0x4000, 0x0A);
        mbc.write_ram(0xA000, 23); // hours
        mbc.write_rom(0x4000, 0x09);
        mbc.write_ram(0xA000, 59); // minutes
        mbc.write_rom(0x4000, 0x08);
        mbc.write_ram(0xA000, 59); // seconds

        // One second later the whole thing rolls over into a new day.
        tick_seconds(&mut mbc, 1);
        assert_eq!(read_clock(&mut mbc, 0x08), 0);
        assert_eq!(read_clock(&mut mbc, 0x09), 0);
        assert_eq!(read_clock(&mut mbc, 0x0A), 0);
        assert_eq!(read_clock(&mut mbc, 0x0B), 1, "day counter advanced");
    }

    #[test]
    fn the_day_counter_is_nine_bits_with_a_carry() {
        let mut mbc = mbc3(16, 0);

        // Set day 511, the last one the counter can hold, at 23:59:59.
        mbc.write_rom(0x4000, 0x0B);
        mbc.write_ram(0xA000, 0xFF);
        mbc.write_rom(0x4000, 0x0C);
        mbc.write_ram(0xA000, 0x01); // day bit 8
        mbc.write_rom(0x4000, 0x0A);
        mbc.write_ram(0xA000, 23);
        mbc.write_rom(0x4000, 0x09);
        mbc.write_ram(0xA000, 59);
        mbc.write_rom(0x4000, 0x08);
        mbc.write_ram(0xA000, 59);

        // DH reads back the day's high bit, with the unwired bits high.
        assert_eq!(read_clock(&mut mbc, 0x0C) & 0x01, 0x01);
        assert_eq!(read_clock(&mut mbc, 0x0C) & 0x80, 0x00, "carry not set yet");

        tick_seconds(&mut mbc, 1);
        assert_eq!(read_clock(&mut mbc, 0x0B), 0, "day counter wrapped");
        let dh = read_clock(&mut mbc, 0x0C);
        assert_eq!(dh & 0x01, 0x00);
        assert_eq!(dh & 0x80, 0x80, "the wrap set the carry");
    }

    #[test]
    fn the_gate_covers_the_clock_too() {
        let mut mbc = mbc3(16, 0);
        tick_seconds(&mut mbc, 5);
        mbc.write_rom(0x6000, 0x00);
        mbc.write_rom(0x6000, 0x01);
        mbc.write_rom(0x4000, 0x08);

        mbc.write_rom(0x0000, 0x00); // shut the gate
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
        mbc.write_rom(0x0000, 0x0A);
        assert_eq!(mbc.read_ram(0xA000), 5);
    }

    #[test]
    fn a_cartridge_without_a_crystal_has_no_clock() {
        let mut mbc = Mbc3::new(banked_rom(16), 16, 8 * 1024, false);
        mbc.write_rom(0x0000, 0x0A);
        mbc.write_rom(0x4000, 0x08);
        // Nothing answers in the clock's place.
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
        tick_seconds(&mut mbc, 5);
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
    }
}
