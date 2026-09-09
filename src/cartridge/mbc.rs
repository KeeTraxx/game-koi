//! Memory bank controllers: the chip on the cartridge board that makes ROMs bigger
//! than 32 KiB possible.
//!
//! The CPU has a 16-bit address bus and gives the cartridge only 32 KiB of it —
//! 0x0000-0x7FFF — plus an 8 KiB window at 0xA000-0xBFFF for save RAM. A 1 MiB game
//! obviously does not fit. The MBC sits between the cartridge's address pins and the
//! ROM chip's, and drives the ROM's *upper* address lines from registers of its own.
//! The CPU picks the low 14 bits; the mapper picks which 16 KiB bank those bits land
//! in.
//!
//! The registers are not memory-mapped in the usual sense. There is nothing to write
//! to at 0x2000 — the board decodes a write in that range as a command and latches the
//! value into a register. The written *address* selects the register and the written
//! *value* is the payload, which is why every mapper write in a game looks like
//! `ld [0x2000], a`.
//!
//! Reads never have side effects here, so [`Mbc::read_rom`] and [`Mbc::read_ram`] take
//! `&self`; only writes touch the registers.

use super::header::{Header, Mapper};

use std::fmt;

/// The size of one ROM bank: the 0x4000-0x7FFF window.
const ROM_BANK_SIZE: usize = 0x4000;
/// The size of one cartridge RAM bank: the 0xA000-0xBFFF window.
const RAM_BANK_SIZE: usize = 0x2000;

/// What the bus sees when it reads a cartridge address nothing answers.
///
/// The data bus floats high when no chip drives it, so an open bus reads as all ones.
/// Disabled save RAM behaves this way, and games rely on it to detect whether RAM is
/// present at all.
const OPEN_BUS: u8 = 0xFF;

/// A cartridge's mapper chip.
///
/// The bus decodes an address to one of these four operations and the mapper decides
/// what it means. Everything about banking lives behind this trait, so the bus never
/// has to know which chip it is talking to.
pub trait Mbc {
    /// Reads from 0x0000-0x7FFF.
    fn read_rom(&self, address: u16) -> u8;
    /// A write to 0x0000-0x7FFF: always a mapper command, never a store.
    fn write_rom(&mut self, address: u16, value: u8);
    /// Reads from 0xA000-0xBFFF.
    fn read_ram(&self, address: u16) -> u8;
    /// Writes to 0xA000-0xBFFF.
    fn write_ram(&mut self, address: u16, value: u8);
}

/// A cartridge whose mapper we do not emulate yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedMapper(pub Mapper);

impl fmt::Display for UnsupportedMapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} cartridges are not supported yet", self.0)
    }
}

impl std::error::Error for UnsupportedMapper {}

/// Builds the mapper a cartridge's header calls for.
///
/// The ROM image is moved in rather than borrowed: the mapper outlives the load, and
/// giving it ownership avoids threading a lifetime through the bus and the CPU.
pub fn for_cartridge(header: &Header, rom: Vec<u8>) -> Result<Box<dyn Mbc>, UnsupportedMapper> {
    let ram = vec![0; header.ram_size];

    match header.cartridge_type.mapper {
        Mapper::None => Ok(Box::new(NoMbc { rom, ram })),
        Mapper::Mbc1 => Ok(Box::new(Mbc1::new(rom, ram, header.rom_banks()))),
        other => Err(UnsupportedMapper(other)),
    }
}

/// No mapper: the ROM chip is wired straight to the bus.
///
/// Cartridge types 0x00, 0x08, and 0x09. At most 32 KiB of ROM, because there are no
/// spare address lines to drive, and optionally a single 8 KiB RAM chip — which has no
/// enable register either, since there is no chip to hold one.
struct NoMbc {
    rom: Vec<u8>,
    ram: Vec<u8>,
}

impl Mbc for NoMbc {
    fn read_rom(&self, address: u16) -> u8 {
        self.rom.get(address as usize).copied().unwrap_or(OPEN_BUS)
    }

    fn write_rom(&mut self, _address: u16, _value: u8) {
        // Nothing to command, and ROM is read-only. Silently dropped, as on hardware.
    }

    fn read_ram(&self, address: u16) -> u8 {
        if self.ram.is_empty() {
            return OPEN_BUS;
        }
        let offset = address as usize & (RAM_BANK_SIZE - 1);
        self.ram[offset % self.ram.len()]
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        if self.ram.is_empty() {
            return;
        }
        let offset = address as usize & (RAM_BANK_SIZE - 1);
        let wrapped = offset % self.ram.len();
        self.ram[wrapped] = value;
    }
}

/// MBC1: the first and most common mapper. Up to 2 MiB of ROM and 32 KiB of RAM.
///
/// Three registers plus an enable latch, all write-only:
///
/// | Range | Register | Width |
/// |---|---|---|
/// | 0x0000-0x1FFF | RAM enable | 4 bits |
/// | 0x2000-0x3FFF | BANK1 | 5 bits |
/// | 0x4000-0x5FFF | BANK2 | 2 bits |
/// | 0x6000-0x7FFF | MODE | 1 bit |
///
/// BANK1 and BANK2 concatenate into a 7-bit ROM bank number (`BANK2:BANK1`), which is
/// what gets to 2 MiB. BANK2 does double duty as the RAM bank number, and MODE decides
/// which job it is doing — see [`Mbc1::rom_bank`].
struct Mbc1 {
    rom: Vec<u8>,
    ram: Vec<u8>,

    /// Save RAM answers only while this is set. Games enable it around a save and
    /// disable it immediately after, so that a crashing or browning-out console
    /// cannot scribble on the save file.
    ram_enabled: bool,

    /// The low 5 bits of the ROM bank number. Never zero; see [`Mbc1::write_rom`].
    bank1: u8,
    /// Two bits that are either the top of the ROM bank number or the RAM bank
    /// number, depending on `mode`.
    bank2: u8,
    /// False = ROM banking mode, true = RAM banking mode.
    mode: bool,

    /// `rom_banks - 1`. Bank numbers are masked with this rather than bounds-checked,
    /// because that is physically what happens: a 256 KiB ROM has only 4 upper address
    /// lines wired, so asking for bank 0x13 on a 16-bank cart gets you bank 0x03. Some
    /// games rely on this wrap.
    rom_bank_mask: usize,
}

impl Mbc1 {
    fn new(rom: Vec<u8>, ram: Vec<u8>, rom_banks: usize) -> Self {
        Mbc1 {
            rom,
            ram,
            ram_enabled: false,
            // Power-on state: BANK1 = 1, so bank 1 sits at 0x4000 before a game
            // writes anything. It cannot be 0 — that is the whole point of the
            // register's zero-adjust.
            bank1: 1,
            bank2: 0,
            mode: false,
            rom_bank_mask: rom_banks.max(1) - 1,
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
    ///   reaches the bottom of each 512 KiB quarter, which is why multicarts use it.
    ///
    /// Because BANK1 is forced non-zero, banks 0x00/0x20/0x40/0x60 can never appear in
    /// the *high* window. A 1 MiB cartridge therefore cannot reach 3 of its 64 banks
    /// through it; real games simply avoid putting anything important there.
    fn rom_bank(&self, address: u16) -> usize {
        let bank = if address < 0x4000 {
            if self.mode { self.bank2 << 5 } else { 0 }
        } else {
            (self.bank2 << 5) | self.bank1
        };
        bank as usize & self.rom_bank_mask
    }

    /// Which RAM bank is currently visible, or bank 0 when BANK2 is busy addressing
    /// ROM. Only 32 KiB carts have more than one bank to choose from.
    fn ram_bank(&self) -> usize {
        if self.mode { self.bank2 as usize } else { 0 }
    }

    /// Where in the RAM chip `address` lands, or `None` if nothing answers.
    ///
    /// The modulo mirrors the chip: a 2 KiB RAM has its top address lines unconnected,
    /// so the same 2 KiB repeats four times across the 8 KiB window. The same wrap
    /// covers a bank number larger than the chip has banks.
    fn ram_offset(&self, address: u16) -> Option<usize> {
        if !self.ram_enabled || self.ram.is_empty() {
            return None;
        }
        let offset = self.ram_bank() * RAM_BANK_SIZE + (address as usize & (RAM_BANK_SIZE - 1));
        Some(offset % self.ram.len())
    }
}

impl Mbc for Mbc1 {
    fn read_rom(&self, address: u16) -> u8 {
        let offset = self.rom_bank(address) * ROM_BANK_SIZE + (address as usize & 0x3FFF);
        self.rom.get(offset).copied().unwrap_or(OPEN_BUS)
    }

    fn write_rom(&mut self, address: u16, value: u8) {
        match address {
            // Any value whose low nibble is 0xA unlocks save RAM; everything else
            // locks it. A single magic value means a wild write is overwhelmingly
            // likely to *disable* RAM rather than expose the save to corruption.
            0x0000..=0x1FFF => self.ram_enabled = value & 0x0F == 0x0A,

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
        match self.ram_offset(address) {
            Some(offset) => self.ram[offset],
            None => OPEN_BUS,
        }
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        // Writes to disabled RAM are dropped, not buffered. That is the protection.
        if let Some(offset) = self.ram_offset(address) {
            self.ram[offset] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM where every byte of bank N is N, so a read tells you which bank answered.
    fn banked_rom(banks: usize) -> Vec<u8> {
        (0..banks)
            .flat_map(|bank| std::iter::repeat_n(bank as u8, ROM_BANK_SIZE))
            .collect()
    }

    fn mbc1(banks: usize, ram_bytes: usize) -> Mbc1 {
        Mbc1::new(banked_rom(banks), vec![0; ram_bytes], banks)
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
    fn bank_numbers_wrap_to_the_chip_size() {
        // 4 banks means only 2 address lines are wired: bank 5 is bank 1.
        let mut mbc = mbc1(4, 0);
        mbc.write_rom(0x2000, 5);
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

    #[test]
    fn a_small_ram_chip_mirrors() {
        // 2 KiB in an 8 KiB window: the top address lines are not connected, so the
        // chip repeats rather than the read failing.
        let mut mbc = mbc1(4, 2 * 1024);
        mbc.write_rom(0x0000, 0x0A);
        mbc.write_ram(0xA000, 0x77);
        assert_eq!(mbc.read_ram(0xA800), 0x77);
        assert_eq!(mbc.read_ram(0xB000), 0x77);
    }

    #[test]
    fn a_cartridge_with_no_ram_reads_open_bus() {
        let mut mbc = mbc1(4, 0);
        mbc.write_rom(0x0000, 0x0A);
        mbc.write_ram(0xA000, 0x42);
        assert_eq!(mbc.read_ram(0xA000), OPEN_BUS);
    }

    #[test]
    fn no_mbc_ignores_mapper_writes() {
        let mut mbc = NoMbc {
            rom: banked_rom(2),
            ram: vec![0; 8 * 1024],
        };
        mbc.write_rom(0x2000, 1);
        // Nothing switched: bank 1 is simply what physically follows bank 0.
        assert_eq!(mbc.read_rom(0x0000), 0);
        assert_eq!(mbc.read_rom(0x4000), 1);

        // Its RAM needs no unlocking, because there is no chip to hold an enable bit.
        mbc.write_ram(0xA000, 0x42);
        assert_eq!(mbc.read_ram(0xA000), 0x42);
    }
}
