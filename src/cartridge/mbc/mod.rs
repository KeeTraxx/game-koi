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
//! # What the chips have in common
//!
//! Every mapper here is the same three ideas in different proportions: a gate that
//! locks save RAM, one or more registers holding a bank number, and the wiring that
//! turns "bank number plus CPU address" into an offset in a chip. [`Rom`] and [`Ram`]
//! hold that last part, so each mapper module is only about its own registers.
//!
//! | | ROM max | RAM | Distinguishing feature |
//! |---|---|---|---|
//! | [`mbc1`] | 2 MiB | 32 KiB | Two bank registers, and a mode bit deciding what the second one means |
//! | [`mbc2`] | 256 KiB | 512 x 4 bits, on-chip | Registers selected by an address bit, not an address range |
//! | [`mbc3`] | 2 MiB | 32 KiB | A real-time clock that keeps running with the console off |
//! | [`mbc5`] | 8 MiB | 128 KiB | 9-bit bank number, and bank 0 is finally selectable |
//!
//! Reads never have side effects here, so [`Mbc::read_rom`] and [`Mbc::read_ram`] take
//! `&self`; only writes touch the registers. The one thing that changes without a write
//! is MBC3's clock, which is why [`Mbc::tick`] exists.

mod mbc1;
mod mbc2;
mod mbc3;
mod mbc5;

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
/// The bus decodes an address to one of these operations and the mapper decides what it
/// means. Everything about banking lives behind this trait, so the bus never has to
/// know which chip it is talking to.
pub trait Mbc {
    /// Reads from 0x0000-0x7FFF.
    fn read_rom(&self, address: u16) -> u8;
    /// A write to 0x0000-0x7FFF: always a mapper command, never a store.
    fn write_rom(&mut self, address: u16, value: u8);
    /// Reads from 0xA000-0xBFFF.
    fn read_ram(&self, address: u16) -> u8;
    /// Writes to 0xA000-0xBFFF.
    fn write_ram(&mut self, address: u16, value: u8);

    /// Advances the mapper by one M-cycle.
    ///
    /// Only MBC3's clock needs this; the default does nothing, so the other mappers do
    /// not have to carry an empty method. The cartridge is on the same clock as the
    /// rest of the machine, so a paused emulator has a paused cartridge.
    fn tick(&mut self) {}
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
    let banks = header.rom_banks();
    let ram_size = header.ram_size;

    match header.cartridge_type.mapper {
        Mapper::None => Ok(Box::new(NoMbc {
            rom: Rom::new(rom, banks),
            ram: Ram::new(ram_size),
        })),
        // MBC1 multicarts share MBC1's type byte and can only be told apart by looking
        // at the image itself; see `mbc1::is_multicart`.
        Mapper::Mbc1 => Ok(Box::new(mbc1::Mbc1::new(rom, banks, ram_size))),
        Mapper::Mbc2 => Ok(Box::new(mbc2::Mbc2::new(rom, banks, ram_size))),
        Mapper::Mbc3 => Ok(Box::new(mbc3::Mbc3::new(
            rom,
            banks,
            ram_size,
            header.cartridge_type.has_timer,
        ))),
        Mapper::Mbc5 => Ok(Box::new(mbc5::Mbc5::new(rom, banks, ram_size))),
        other => Err(UnsupportedMapper(other)),
    }
}

/// The ROM chip, plus how many of its address lines the mapper actually drives.
///
/// Bank numbers are *masked*, not bounds-checked. That is what the hardware does: a 256
/// KiB ROM has only four upper address lines wired, so asking for bank 0x13 on a
/// 16-bank cart gets you bank 0x03. Some games rely on the wrap, and Mooneye's
/// `rom_*` tests check for it specifically.
struct Rom {
    bytes: Vec<u8>,
    /// `banks - 1`, which works because every real ROM chip is a power-of-two size.
    bank_mask: usize,
}

impl Rom {
    fn new(bytes: Vec<u8>, banks: usize) -> Self {
        Rom {
            bytes,
            bank_mask: banks.next_power_of_two().max(1) - 1,
        }
    }

    /// Reads `address` from `bank`. Only the low 14 bits of the address are used, so
    /// callers can pass either window's address unchanged.
    fn read(&self, bank: usize, address: u16) -> u8 {
        let offset = (bank & self.bank_mask) * ROM_BANK_SIZE + (address as usize & 0x3FFF);
        self.bytes.get(offset).copied().unwrap_or(OPEN_BUS)
    }
}

/// The cartridge's save RAM and the gate in front of it.
///
/// The gate is not a convenience: games open it around a save and shut it immediately
/// after, so that a console browning out mid-write cannot scribble on the save. Every
/// mapper implements it, and Mooneye's `bits_ramg` tests check exactly which written
/// values open it — which is *not* the same on every chip.
struct Ram {
    bytes: Vec<u8>,
    enabled: bool,
}

impl Ram {
    fn new(size: usize) -> Self {
        Ram {
            bytes: vec![0; size],
            enabled: false,
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn is_present(&self) -> bool {
        !self.bytes.is_empty()
    }

    /// Where `address` in `bank` lands in the chip, or `None` if nothing answers.
    ///
    /// The modulo mirrors the chip, because the address lines it does not have simply
    /// are not connected: 2 KiB of RAM repeats four times across the 8 KiB window, and
    /// a bank number past the end of the chip wraps to the start.
    fn offset(&self, bank: usize, address: u16) -> Option<usize> {
        if !self.enabled || self.bytes.is_empty() {
            return None;
        }
        let offset = bank * RAM_BANK_SIZE + (address as usize & (RAM_BANK_SIZE - 1));
        Some(offset % self.bytes.len())
    }

    fn read(&self, bank: usize, address: u16) -> u8 {
        match self.offset(bank, address) {
            Some(offset) => self.bytes[offset],
            None => OPEN_BUS,
        }
    }

    /// Writes to disabled or absent RAM are dropped, not buffered. That is the whole
    /// protection the gate provides.
    fn write(&mut self, bank: usize, address: u16, value: u8) {
        if let Some(offset) = self.offset(bank, address) {
            self.bytes[offset] = value;
        }
    }
}

/// No mapper: the ROM chip is wired straight to the bus.
///
/// Cartridge types 0x00, 0x08, and 0x09. At most 32 KiB of ROM, because there are no
/// spare address lines to drive, and optionally a single 8 KiB RAM chip — which has no
/// enable register either, since there is no chip to hold one, so the gate is simply
/// open from the start.
struct NoMbc {
    rom: Rom,
    ram: Ram,
}

impl Mbc for NoMbc {
    fn read_rom(&self, address: u16) -> u8 {
        // Bank 0 for both windows: the 32 KiB image is addressed flat, so bank 1 is
        // just what physically follows bank 0.
        self.rom
            .bytes
            .get(address as usize)
            .copied()
            .unwrap_or(OPEN_BUS)
    }

    fn write_rom(&mut self, _address: u16, _value: u8) {
        // Nothing to command, and ROM is read-only. Silently dropped, as on hardware.
    }

    fn read_ram(&self, address: u16) -> u8 {
        if !self.ram.is_present() {
            return OPEN_BUS;
        }
        let offset = address as usize & (RAM_BANK_SIZE - 1);
        self.ram.bytes[offset % self.ram.bytes.len()]
    }

    fn write_ram(&mut self, address: u16, value: u8) {
        if !self.ram.is_present() {
            return;
        }
        let offset = address as usize & (RAM_BANK_SIZE - 1);
        let wrapped = offset % self.ram.bytes.len();
        self.ram.bytes[wrapped] = value;
    }
}

/// A ROM where every byte of bank N is N, so a read tells you which bank answered.
///
/// Shared by every mapper's tests. Bank numbers above 255 wrap in the fill byte, which
/// only matters for MBC5's 512-bank sizes — its tests check the bank arithmetic
/// directly instead.
#[cfg(test)]
fn banked_rom(banks: usize) -> Vec<u8> {
    (0..banks)
        .flat_map(|bank| std::iter::repeat_n(bank as u8, ROM_BANK_SIZE))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_bank_numbers_wrap_to_the_chip_size() {
        // 4 banks means only 2 address lines are wired: bank 5 is bank 1.
        let rom = Rom::new(banked_rom(4), 4);
        assert_eq!(rom.read(1, 0x4000), 1);
        assert_eq!(rom.read(5, 0x4000), 1);
        assert_eq!(rom.read(0xFF, 0x4000), 3);
    }

    #[test]
    fn ram_is_shut_until_opened() {
        let mut ram = Ram::new(8 * 1024);
        ram.write(0, 0xA000, 0x42);
        assert_eq!(ram.read(0, 0xA000), OPEN_BUS);

        ram.set_enabled(true);
        ram.write(0, 0xA000, 0x42);
        assert_eq!(ram.read(0, 0xA000), 0x42);
    }

    #[test]
    fn a_small_ram_chip_mirrors() {
        // 2 KiB in an 8 KiB window: the top address lines are not connected, so the
        // chip repeats rather than the read failing.
        let mut ram = Ram::new(2 * 1024);
        ram.set_enabled(true);
        ram.write(0, 0xA000, 0x77);
        assert_eq!(ram.read(0, 0xA800), 0x77);
        assert_eq!(ram.read(0, 0xB000), 0x77);
    }

    #[test]
    fn absent_ram_reads_open_bus() {
        let mut ram = Ram::new(0);
        ram.set_enabled(true);
        ram.write(0, 0xA000, 0x42);
        assert_eq!(ram.read(0, 0xA000), OPEN_BUS);
    }

    #[test]
    fn no_mbc_ignores_mapper_writes() {
        let mut mbc = NoMbc {
            rom: Rom::new(banked_rom(2), 2),
            ram: Ram::new(8 * 1024),
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
