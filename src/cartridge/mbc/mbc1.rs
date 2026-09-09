//! MBC1: the first and most common mapper. Up to 2 MiB of ROM and 32 KiB of RAM.
//!
//! Three registers plus an enable gate, all write-only:
//!
//! | Range | Register | Width |
//! |---|---|---|
//! | 0x0000-0x1FFF | RAMG (RAM gate) | 4 bits |
//! | 0x2000-0x3FFF | BANK1 | 5 bits |
//! | 0x4000-0x5FFF | BANK2 | 2 bits |
//! | 0x6000-0x7FFF | MODE | 1 bit |
//!
//! BANK1 and BANK2 concatenate into a 7-bit ROM bank number (`BANK2:BANK1`), which is
//! what reaches 2 MiB. BANK2 does double duty as the RAM bank number, and MODE decides
//! which job it is doing — see [`Mbc1::rom_bank`].

use super::{Mbc, Ram, Rom};
use crate::cartridge::header;

/// A "game" slot in a multicart: 16 banks, 256 KiB.
const MULTICART_GAME_BANKS: usize = 16;

pub(super) struct Mbc1 {
    rom: Rom,
    ram: Ram,

    /// The low 5 bits of the ROM bank number. Never zero; see [`Mbc1::write_rom`].
    bank1: u8,
    /// Two bits that are either the top of the ROM bank number or the RAM bank
    /// number, depending on `mode`.
    bank2: u8,
    /// False = ROM banking mode, true = RAM banking mode.
    mode: bool,

    /// Multicart wiring, where BANK1's bit 4 is not connected to anything.
    multicart: bool,
}

impl Mbc1 {
    pub(super) fn new(rom: Vec<u8>, rom_banks: usize, ram_size: usize) -> Self {
        let multicart = is_multicart(&rom);
        Mbc1 {
            rom: Rom::new(rom, rom_banks),
            ram: Ram::new(ram_size),
            // Power-on state: BANK1 = 1, so bank 1 sits at 0x4000 before a game writes
            // anything. It cannot be 0 — that is the whole point of the zero-adjust.
            bank1: 1,
            bank2: 0,
            mode: false,
            multicart,
        }
    }

    /// Which ROM bank is currently visible at `address`.
    ///
    /// The two windows behave differently, and this is the part of MBC1 that surprises
    /// people:
    ///
    /// - **0x4000-0x7FFF** always shows `BANK2:BANK1`.
    /// - **0x0000-0x3FFF** shows bank 0 in mode 0, but in mode 1 it shows
    ///   `BANK2:00000` — bank 0x00, 0x20, 0x40, or 0x60. Mode 1 is how a large cart
    ///   reaches the bottom of each 512 KiB quarter, which is what multicarts use to
    ///   put a different game's bank 0 in front of the CPU.
    ///
    /// Because BANK1 is forced non-zero, banks 0x00/0x20/0x40/0x60 can never appear in
    /// the *high* window. A 1 MiB cartridge therefore cannot reach 3 of its 64 banks
    /// through it; real games simply avoid putting anything important there.
    fn rom_bank(&self, address: u16) -> usize {
        // On a multicart BANK1's bit 4 is not wired, so the bank number is 6 bits and
        // BANK2 shifts up by 4 instead of 5. BANK2 then works out to a game number and
        // BANK1 to a bank within that game.
        let (bank1, shift) = if self.multicart {
            (self.bank1 & 0x0F, 4)
        } else {
            (self.bank1, 5)
        };

        let bank = if address < 0x4000 {
            if self.mode { self.bank2 << shift } else { 0 }
        } else {
            (self.bank2 << shift) | bank1
        };
        bank as usize
    }

    /// Which RAM bank is currently visible, or bank 0 when BANK2 is busy addressing
    /// ROM. Only 32 KiB carts have more than one bank to choose from.
    fn ram_bank(&self) -> usize {
        if self.mode { self.bank2 as usize } else { 0 }
    }
}

impl Mbc for Mbc1 {
    fn read_rom(&self, address: u16) -> u8 {
        self.rom.read(self.rom_bank(address), address)
    }

    fn write_rom(&mut self, address: u16, value: u8) {
        match address {
            // RAMG. Any value whose low nibble is 0xA opens the gate; everything else
            // shuts it. Decoding a single bit pattern means a wild write is
            // overwhelmingly likely to *shut* the gate rather than expose the save.
            0x0000..=0x1FFF => self.ram.set_enabled(value & 0x0F == 0x0A),

            // BANK1. Only 5 bits are wired, and a written 0 becomes 1: the register
            // drives ROM address lines A14-A18, and bank 0 is already hard-wired into
            // the low window, so allowing 0 here would just mirror it. This adjust
            // happens on the register itself, before BANK2 is prepended — which is
            // exactly why bank 0x20 is unreachable and bank 0x21 is not.
            0x2000..=0x3FFF => {
                let bank = value & 0x1F;
                self.bank1 = if bank == 0 { 1 } else { bank };
            }

            // BANK2: two bits, no zero-adjust. Its meaning depends on MODE.
            0x4000..=0x5FFF => self.bank2 = value & 0x03,

            // MODE. Games with 32 KiB of save RAM flip this to 1 to reach their upper
            // RAM banks, then usually flip it back before touching ROM.
            0x6000..=0x7FFF => self.mode = value & 0x01 != 0,

            _ => unreachable!("write_rom called outside 0x0000-0x7FFF"),
        }
    }

    fn read_ram(&self, address: u16) -> u8 {
        self.ram.read(self.ram_bank(), address)
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        self.ram.write(self.ram_bank(), address, value);
    }
}

/// Guesses whether a ROM image is a multicart board rather than a plain MBC1.
///
/// There is nothing in the header to go on: a multicart's type byte is an ordinary
/// MBC1 value, because the chip really is an ordinary MBC1 — only the board wiring
/// differs. What gives it away is the contents. Each bundled game is a complete
/// cartridge image with its own header, and they are laid out 256 KiB apart, so a
/// multicart has a Nintendo logo at 0x00104, 0x40104, 0x80104, and 0xC0104 instead of
/// just the first.
///
/// Requiring three of the four rather than all four follows the reference: a cart may
/// bundle only two games plus the menu, leaving the last slot empty. All known
/// multicarts are exactly 1 MiB, so smaller images are not even considered — which is
/// what keeps an ordinary game that happens to embed a logo from being misread.
fn is_multicart(rom: &[u8]) -> bool {
    const MULTICART_SIZE: usize = 1024 * 1024;

    if rom.len() != MULTICART_SIZE {
        return false;
    }

    let logos = (0..4)
        .filter(|slot| header::has_logo_at(rom, slot * MULTICART_GAME_BANKS * 0x4000))
        .count();
    logos >= 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::mbc::{OPEN_BUS, banked_rom};

    fn mbc1(banks: usize, ram_bytes: usize) -> Mbc1 {
        Mbc1::new(banked_rom(banks), banks, ram_bytes)
    }

    #[test]
    fn low_window_is_bank_zero_and_high_window_starts_at_one() {
        let mbc = mbc1(4, 0);
        assert_eq!(mbc.read_rom(0x0000), 0);
        assert_eq!(mbc.read_rom(0x3FFF), 0);
        // Power-on BANK1 is 1, not 0: bank 1 is visible before the game writes.
        assert_eq!(mbc.read_rom(0x4000), 1);
    }

    #[test]
    fn bank1_selects_the_high_window() {
        let mut mbc = mbc1(8, 0);
        mbc.write_rom(0x2000, 5);
        assert_eq!(mbc.read_rom(0x4000), 5);
        assert_eq!(mbc.read_rom(0x7FFF), 5);
        // The low window does not move with it.
        assert_eq!(mbc.read_rom(0x0000), 0);
    }

    #[test]
    fn bank_zero_becomes_bank_one() {
        let mut mbc = mbc1(8, 0);
        mbc.write_rom(0x2000, 0);
        // Not bank 0 — the register cannot hold zero.
        assert_eq!(mbc.read_rom(0x4000), 1);
    }

    #[test]
    fn bank2_supplies_the_upper_bank_bits() {
        // 128 banks = 2 MiB, the largest MBC1 cartridge.
        let mut mbc = mbc1(128, 0);
        mbc.write_rom(0x2000, 0x01);
        mbc.write_rom(0x4000, 0x02);
        // BANK2:BANK1 = 0b10_00001 = 0x41.
        assert_eq!(mbc.read_rom(0x4000), 0x41);
    }

    #[test]
    fn the_gap_banks_are_unreachable_in_the_high_window() {
        // The classic MBC1 quirk. BANK1 = 0 becomes 1 *before* BANK2 is prepended, so
        // asking for 0x20 yields 0x21 and bank 0x20 can never be selected here.
        let mut mbc = mbc1(128, 0);
        for (bank2, unreachable) in [(1, 0x20), (2, 0x40), (3, 0x60)] {
            mbc.write_rom(0x4000, bank2);
            mbc.write_rom(0x2000, 0);
            assert_eq!(
                mbc.read_rom(0x4000),
                unreachable + 1,
                "bank {unreachable:#04X} must not be selectable"
            );
        }
    }

    #[test]
    fn mode_one_banks_the_low_window_too() {
        let mut mbc = mbc1(128, 0);
        mbc.write_rom(0x4000, 0x02);

        // Mode 0: the low window is pinned to bank 0 regardless of BANK2.
        assert_eq!(mbc.read_rom(0x0000), 0);

        // Mode 1: it shows BANK2:00000 = 0x40 — how a big cart reaches the bottom of
        // each quarter, and the only way to see the gap banks at all.
        mbc.write_rom(0x6000, 1);
        assert_eq!(mbc.read_rom(0x0000), 0x40);
        // The high window is unaffected by the mode change.
        assert_eq!(mbc.read_rom(0x4000), 0x41);
    }

    #[test]
    fn ram_is_locked_until_the_magic_value_is_written() {
        let mut mbc = mbc1(4, 8 * 1024);

        // Locked at power-on: writes are dropped and reads float high.
        mbc.write_ram(0xA000, 0x42);
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);

        mbc.write_rom(0x0000, 0x0A);
        mbc.write_ram(0xA000, 0x42);
        assert_eq!(mbc.read_ram(0xA000), 0x42);

        // Any low nibble other than 0xA locks it again; the data survives, hidden.
        mbc.write_rom(0x0000, 0x00);
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
        mbc.write_rom(0x0000, 0x1A);
        assert_eq!(mbc.read_ram(0xA000), 0x42, "only the low nibble is decoded");
    }

    #[test]
    fn ram_banking_needs_mode_one() {
        let mut mbc = mbc1(4, 32 * 1024);
        mbc.write_rom(0x0000, 0x0A);
        mbc.write_rom(0x6000, 1);

        for bank in 0..4u8 {
            mbc.write_rom(0x4000, bank);
            mbc.write_ram(0xA000, 0xB0 | bank);
        }
        for bank in 0..4u8 {
            mbc.write_rom(0x4000, bank);
            assert_eq!(
                mbc.read_ram(0xA000),
                0xB0 | bank,
                "bank {bank} kept its byte"
            );
        }

        // Back in mode 0, BANK2 addresses ROM instead and RAM is stuck on bank 0.
        mbc.write_rom(0x6000, 0);
        mbc.write_rom(0x4000, 3);
        assert_eq!(mbc.read_ram(0xA000), 0xB0);
    }

    /// Builds a 1 MiB image with a Nintendo logo at the start of `games` of the four
    /// 256 KiB slots, which is what multicart detection looks for.
    fn multicart_image(games: usize) -> Vec<u8> {
        let mut rom = banked_rom(64);
        for slot in 0..games {
            header::write_logo_at(&mut rom, slot * MULTICART_GAME_BANKS * 0x4000);
        }
        rom
    }

    #[test]
    fn a_plain_cartridge_is_not_mistaken_for_a_multicart() {
        // One logo, in the ordinary place: an ordinary 1 MiB game.
        assert!(!is_multicart(&multicart_image(1)));
        // Two is still not enough — the reference wants most of the four slots.
        assert!(!is_multicart(&multicart_image(2)));
        // And a cart of the wrong size is never considered, whatever it contains.
        assert!(!is_multicart(&banked_rom(4)));
    }

    #[test]
    fn three_logos_mark_a_multicart() {
        // Menu plus two games: the reference notes the fourth slot may be empty.
        assert!(is_multicart(&multicart_image(3)));
        assert!(is_multicart(&multicart_image(4)));
    }

    #[test]
    fn a_multicart_skips_bit_four_of_bank1() {
        let mut mbc = Mbc1::new(multicart_image(4), 64, 0);
        assert!(
            mbc.multicart,
            "test image should be detected as a multicart"
        );

        // Game 3, bank 0x1D within it. Bit 4 of BANK1 is not connected, so the 0x10 in
        // 0x1D is dropped and BANK2 lands at bit 4 instead of bit 5:
        // BANK2:BANK1<3:0> = 0b11_1101 = 0x3D. On a plain MBC1 the same writes would
        // select 0b11_11101 = 0x7D.
        mbc.write_rom(0x4000, 3);
        mbc.write_rom(0x2000, 0x1D);
        assert_eq!(mbc.read_rom(0x4000), 0x3D);

        // Mode 1 puts that game's own bank 0 in the low window — how the menu hands
        // control to a game that thinks it is alone on the cartridge.
        mbc.write_rom(0x6000, 1);
        assert_eq!(mbc.read_rom(0x0000), 0x30);
    }
}
