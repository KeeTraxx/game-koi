//! MBC5: up to 8 MiB of ROM and 128 KiB of RAM. Most Game Boy Color games use it.
//!
//! Four registers, and the design reads like a list of MBC1's annoyances being fixed:
//!
//! | Range | Register | Width |
//! |---|---|---|
//! | 0x0000-0x1FFF | RAMG (RAM gate) | 8 bits |
//! | 0x2000-0x2FFF | ROMB0 (bank, low bits) | 8 bits |
//! | 0x3000-0x3FFF | ROMB1 (bank, bit 8) | 1 bit |
//! | 0x4000-0x5FFF | RAMB (RAM bank) | 4 bits |
//!
//! **Bank 0 is selectable.** No zero-adjust: writing 0 to ROMB0 really does put bank 0
//! in the high window, mirroring the low one. MBC1's whole 0x20/0x40/0x60 gap comes from
//! the missing version of this, and it is gone here.
//!
//! **The bank number is one register split in two,** not two registers meaning different
//! things in different modes. Nine bits reach 512 banks, and there is no mode bit at all.
//!
//! One thing did get *stricter*: the RAM gate compares all eight bits, so only exactly
//! 0x0A opens it. On MBC1 and MBC2 the gate decodes the low nibble alone, and 0x1A works
//! as well as 0x0A. Per gekkio's reference this is a real difference between the chips,
//! not a documentation slip.
//!
//! On rumble cartridges, bit 3 of RAMB is wired to the motor instead of to the RAM chip.
//! We do not drive a motor, and those carts have at most 8 banks of RAM, so the bit
//! simply addresses nothing.

use super::{Mbc, Ram, Rom};

pub(super) struct Mbc5 {
    rom: Rom,
    ram: Ram,
    /// The low 8 bits of the ROM bank number.
    romb0: u8,
    /// Bit 8 of the ROM bank number. Only one bit of the written byte is wired.
    romb1: u8,
    /// RAM bank, 4 bits.
    ramb: u8,
}

impl Mbc5 {
    pub(super) fn new(rom: Vec<u8>, rom_banks: usize, ram_size: usize) -> Self {
        Mbc5 {
            rom: Rom::new(rom, rom_banks),
            ram: Ram::new(ram_size),
            // Power-on bank is 1, but only because that is the reset value of the
            // register — not because 0 is forbidden. A game may write 0 whenever it
            // likes.
            romb0: 1,
            romb1: 0,
            ramb: 0,
        }
    }

    fn rom_bank(&self) -> usize {
        ((self.romb1 as usize) << 8) | self.romb0 as usize
    }
}

impl Mbc for Mbc5 {
    fn ram(&self) -> Option<&Ram> {
        Some(&self.ram)
    }

    fn ram_mut(&mut self) -> Option<&mut Ram> {
        Some(&mut self.ram)
    }

    fn read_rom(&self, address: u16) -> u8 {
        let bank = if address < 0x4000 { 0 } else { self.rom_bank() };
        self.rom.read(bank, address)
    }

    fn write_rom(&mut self, address: u16, value: u8) {
        match address {
            // All eight bits are compared here, unlike MBC1's low-nibble decode.
            0x0000..=0x1FFF => self.ram.set_enabled(value == 0x0A),

            // The two halves of the bank number. Splitting 0x2000-0x3FFF into two 4 KiB
            // ranges is what buys the ninth bit without spending another register slot.
            0x2000..=0x2FFF => self.romb0 = value,
            0x3000..=0x3FFF => self.romb1 = value & 0x01,

            0x4000..=0x5FFF => self.ramb = value & 0x0F,

            // 0x6000-0x7FFF is not a register on MBC5: there is no mode to select and
            // no clock to latch.
            0x6000..=0x7FFF => {}

            _ => unreachable!("write_rom called outside 0x0000-0x7FFF"),
        }
    }

    fn read_ram(&self, address: u16) -> u8 {
        self.ram.read(self.ramb as usize, address)
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        self.ram.write(self.ramb as usize, address, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::mbc::{OPEN_BUS, banked_rom};

    fn mbc5(banks: usize, ram_bytes: usize) -> Mbc5 {
        Mbc5::new(banked_rom(banks), banks, ram_bytes)
    }

    #[test]
    fn bank_zero_is_selectable() {
        // The headline difference from every earlier mapper.
        let mut mbc = mbc5(16, 0);
        mbc.write_rom(0x2000, 0);
        assert_eq!(mbc.rom_bank(), 0);
        assert_eq!(
            mbc.read_rom(0x4000),
            0,
            "bank 0 mirrored into the high window"
        );
    }

    #[test]
    fn the_bank_number_spans_two_registers() {
        let mut mbc = mbc5(512, 0);

        // Low byte alone reaches bank 255.
        mbc.write_rom(0x2000, 0xFF);
        assert_eq!(mbc.rom_bank(), 0xFF);

        // The 0x3000 range adds bit 8, for banks 256-511.
        mbc.write_rom(0x3000, 0x01);
        assert_eq!(mbc.rom_bank(), 0x1FF);

        mbc.write_rom(0x2000, 0x00);
        assert_eq!(mbc.rom_bank(), 0x100);
    }

    #[test]
    fn only_one_bit_of_the_high_register_is_wired() {
        let mut mbc = mbc5(512, 0);
        mbc.write_rom(0x2000, 0x00);
        mbc.write_rom(0x3000, 0xFE);
        assert_eq!(mbc.rom_bank(), 0x000, "bits 7-1 are ignored");
        mbc.write_rom(0x3000, 0xFF);
        assert_eq!(mbc.rom_bank(), 0x100, "only bit 0 reaches the bank number");
    }

    #[test]
    fn the_ram_gate_compares_all_eight_bits() {
        let mut mbc = mbc5(16, 8 * 1024);

        // The value MBC1 would accept as an alias does nothing here.
        mbc.write_rom(0x0000, 0x1A);
        mbc.write_ram(0xA000, 0x42);
        assert_eq!(
            mbc.read_ram(0xA000),
            OPEN_BUS,
            "0x1A must not open the gate"
        );

        mbc.write_rom(0x0000, 0x0A);
        mbc.write_ram(0xA000, 0x42);
        assert_eq!(mbc.read_ram(0xA000), 0x42);
    }

    #[test]
    fn ram_banks_switch_with_no_mode_bit() {
        // 128 KiB: 16 banks, the largest MBC5 RAM.
        let mut mbc = mbc5(16, 128 * 1024);
        mbc.write_rom(0x0000, 0x0A);

        for bank in 0..16u8 {
            mbc.write_rom(0x4000, bank);
            mbc.write_ram(0xA000, 0xD0 | bank);
        }
        for bank in 0..16u8 {
            mbc.write_rom(0x4000, bank);
            assert_eq!(
                mbc.read_ram(0xA000),
                0xD0 | bank,
                "bank {bank} kept its byte"
            );
        }
    }

    #[test]
    fn nothing_above_5fff_is_a_register() {
        let mut mbc = mbc5(16, 0);
        mbc.write_rom(0x2000, 7);
        mbc.write_rom(0x6000, 0x01);
        mbc.write_rom(0x7FFF, 0xFF);
        assert_eq!(mbc.rom_bank(), 7, "no mode register to disturb the bank");
    }
}
