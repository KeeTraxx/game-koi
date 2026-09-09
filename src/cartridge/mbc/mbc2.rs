//! MBC2: the odd one out. Up to 256 KiB of ROM and 512 *nibbles* of RAM.
//!
//! Two things make MBC2 unusual, and both come from the same decision — putting the
//! save RAM inside the mapper chip instead of on a separate chip.
//!
//! **The RAM is 4 bits wide.** 512 addresses of 4 bits each, and the upper four data
//! lines are not connected to anything. A write throws away the high nibble, and a read
//! leaves those lines floating. Games that use it store one nibble per address and mask
//! on the way out.
//!
//! **The registers are selected by an address bit, not an address range.** Every other
//! mapper carves 0x0000-0x7FFF into 8 KiB slices, one per register. MBC2 has only two
//! registers and picks between them with address line A8, so they interleave in 256-byte
//! stripes: 0x0000, 0x2000 and 0x3000 are RAMG, while 0x0100, 0x2100 and 0x3100 are
//! ROMB. Nothing above 0x3FFF is a register at all.
//!
//! Gekkio's reference wonders aloud why MBC2 exists at all, given MBC1 is strictly more
//! capable — the guess being that on-chip RAM looked cheaper, and evidently did not
//! work out, since every later mapper went back to a separate chip.

use super::{Mbc, OPEN_BUS, Ram, Rom};

/// The chip holds 512 addresses, so only A0-A8 of the RAM address matter.
const RAM_ADDRESSES: usize = 512;

pub(super) struct Mbc2 {
    rom: Rom,
    ram: Ram,
    /// ROM bank for the high window: 4 bits, and never zero.
    romb: u8,
}

impl Mbc2 {
    pub(super) fn new(rom: Vec<u8>, rom_banks: usize, _ram_size: usize) -> Self {
        Mbc2 {
            rom: Rom::new(rom, rom_banks),
            // The header's RAM-size byte is always 0 on an MBC2 cart, because the RAM
            // is not on the board to be described. Its size is a property of the chip.
            ram: Ram::new(RAM_ADDRESSES),
            romb: 1,
        }
    }
}

impl Mbc for Mbc2 {
    fn ram_bytes(&self) -> &[u8] {
        self.ram.bytes()
    }

    fn read_rom(&self, address: u16) -> u8 {
        let bank = if address < 0x4000 {
            0
        } else {
            self.romb as usize
        };
        self.rom.read(bank, address)
    }

    fn write_rom(&mut self, address: u16, value: u8) {
        // Only the low half of the ROM area holds registers; 0x4000-0x7FFF is inert.
        if address >= 0x4000 {
            return;
        }

        // A8 picks the register. This is the whole decode — which is why the two
        // registers alternate every 256 bytes across the entire 16 KiB range.
        if address & 0x0100 == 0 {
            // RAMG, four bits, same 0b1010 pattern as MBC1's gate.
            self.ram.set_enabled(value & 0x0F == 0x0A);
        } else {
            // ROMB, four bits, zero-adjusted for the same reason as MBC1's BANK1:
            // bank 0 is already hard-wired into the low window.
            let bank = value & 0x0F;
            self.romb = if bank == 0 { 1 } else { bank };
        }
    }

    fn read_ram(&self, address: u16) -> u8 {
        if self.ram.offset(0, address).is_none() {
            return OPEN_BUS;
        }
        // The high nibble is not stored anywhere — those four data lines float, so
        // they read as ones like any undriven bus line.
        self.ram.read(0, address) | 0xF0
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        // Keep only what the chip can physically hold, so a later read cannot return a
        // high nibble that no hardware would have kept.
        self.ram.write(0, address, value & 0x0F);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::mbc::banked_rom;

    fn mbc2(banks: usize) -> Mbc2 {
        Mbc2::new(banked_rom(banks), banks, 0)
    }

    #[test]
    fn a8_picks_the_register() {
        let mut mbc = mbc2(16);

        // A8 clear: RAMG. The value 0x0A opens the gate and does not touch the bank.
        mbc.write_rom(0x0000, 0x0A);
        assert_eq!(mbc.read_rom(0x4000), 1, "bank unchanged by a RAMG write");
        mbc.write_ram(0xA000, 0x05);
        assert_eq!(mbc.read_ram(0xA000), 0xF5);

        // A8 set: ROMB. Same value, entirely different effect.
        mbc.write_rom(0x0100, 0x0A);
        assert_eq!(mbc.read_rom(0x4000), 0x0A);
    }

    #[test]
    fn the_registers_repeat_every_256_bytes() {
        let mut mbc = mbc2(16);
        // 0x2100 and 0x3100 are ROMB just as much as 0x0100 is.
        mbc.write_rom(0x2100, 3);
        assert_eq!(mbc.read_rom(0x4000), 3);
        mbc.write_rom(0x3100, 7);
        assert_eq!(mbc.read_rom(0x4000), 7);

        // ...and 0x2000/0x3000 are RAMG, so they must shut the gate rather than bank.
        mbc.write_rom(0x0100, 5);
        mbc.write_rom(0x3000, 0x00);
        assert_eq!(mbc.read_rom(0x4000), 5, "a RAMG write left the bank alone");
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
    }

    #[test]
    fn nothing_above_3fff_is_a_register() {
        let mut mbc = mbc2(16);
        mbc.write_rom(0x4100, 7);
        mbc.write_rom(0x6100, 7);
        assert_eq!(mbc.read_rom(0x4000), 1, "the bank register did not move");
    }

    #[test]
    fn bank_zero_becomes_bank_one() {
        let mut mbc = mbc2(16);
        mbc.write_rom(0x0100, 0);
        assert_eq!(mbc.read_rom(0x4000), 1);
        // The low window is bank 0 regardless.
        assert_eq!(mbc.read_rom(0x0000), 0);
    }

    #[test]
    fn only_four_bank_bits_are_wired() {
        let mut mbc = mbc2(16);
        // 0x1F would be bank 31 on MBC1; MBC2 keeps only the low nibble.
        mbc.write_rom(0x0100, 0x1F);
        assert_eq!(mbc.read_rom(0x4000), 0x0F);
    }

    #[test]
    fn ram_keeps_only_the_low_nibble() {
        let mut mbc = mbc2(16);
        mbc.write_rom(0x0000, 0x0A);

        mbc.write_ram(0xA000, 0xFF);
        // Low nibble stored, high nibble floating high — not the 0xFF we wrote back by
        // accident, but the same value for a different reason. Use a distinguishing
        // pattern to be sure:
        mbc.write_ram(0xA001, 0x37);
        assert_eq!(mbc.read_ram(0xA001), 0xF7);
    }

    #[test]
    fn ram_wraps_every_512_bytes() {
        let mut mbc = mbc2(16);
        mbc.write_rom(0x0000, 0x0A);

        mbc.write_ram(0xA000, 0x03);
        // Only A0-A8 reach the chip, so 0xA200 is the same cell as 0xA000.
        assert_eq!(mbc.read_ram(0xA200), 0xF3);
        assert_eq!(mbc.read_ram(0xA400), 0xF3);
        // And so is the far end of the window.
        assert_eq!(mbc.read_ram(0xBE00), 0xF3);
    }

    #[test]
    fn ram_is_shut_at_power_on() {
        let mut mbc = mbc2(16);
        mbc.write_ram(0xA000, 0x05);
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
    }
}
