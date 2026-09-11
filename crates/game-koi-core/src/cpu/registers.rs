//! The SM83 register file.
//!
//! The CPU has eight 8-bit registers (A, F, B, C, D, E, H, L) plus the 16-bit PC and
//! SP. The 8-bit ones pair up as AF, BC, DE, and HL, and a lot of instructions treat
//! a pair as a single 16-bit value — usually as an address. Pairing is big-endian:
//! the first-named register is the high byte.

use std::fmt;

/// The flags register, F.
///
/// Only the top four bits exist as real storage; the low nibble is hardwired to zero
/// and stays zero even if you `POP AF` a value with those bits set. Emulating that
/// masking matters — Blargg's test ROMs check it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// Z — set when the result of an operation was zero.
    pub zero: bool,
    /// N — set when the last operation was a subtraction. Only DAA reads it.
    pub negative: bool,
    /// H — carry out of bit 3 into bit 4. Only DAA reads it.
    pub half_carry: bool,
    /// C — carry out of bit 7 (or bit 15 for 16-bit adds).
    pub carry: bool,
}

impl Flags {
    const ZERO: u8 = 1 << 7;
    const NEGATIVE: u8 = 1 << 6;
    const HALF_CARRY: u8 = 1 << 5;
    const CARRY: u8 = 1 << 4;

    pub fn bits(self) -> u8 {
        let mut bits = 0;
        if self.zero {
            bits |= Self::ZERO;
        }
        if self.negative {
            bits |= Self::NEGATIVE;
        }
        if self.half_carry {
            bits |= Self::HALF_CARRY;
        }
        if self.carry {
            bits |= Self::CARRY;
        }
        bits
    }

    /// Rebuilds flags from a raw byte, discarding the low nibble as the hardware does.
    pub fn from_bits(bits: u8) -> Self {
        Flags {
            zero: bits & Self::ZERO != 0,
            negative: bits & Self::NEGATIVE != 0,
            half_carry: bits & Self::HALF_CARRY != 0,
            carry: bits & Self::CARRY != 0,
        }
    }
}

/// One of the 8-bit registers addressable by the 3-bit `r` field in an opcode.
///
/// The encoding 0-7 is B, C, D, E, H, L, (HL), A. Slot 6 is not a register at all but
/// a memory access through HL, which is why it is absent here and handled separately
/// by the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg8 {
    A,
    B,
    C,
    D,
    E,
    H,
    L,
}

/// A 16-bit register or register pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg16 {
    AF,
    BC,
    DE,
    HL,
    SP,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Registers {
    pub a: u8,
    pub f: Flags,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub pc: u16,
    pub sp: u16,
}

impl Registers {
    /// Post-boot register state for a DMG.
    ///
    /// The boot ROM leaves these values behind when it hands control to the
    /// cartridge at 0x0100. Since we don't run a boot ROM, we start here instead —
    /// games do rely on some of it, and test ROMs assume it.
    ///
    /// A=0x01 identifies the hardware model, and the F value of 0xB0 is a side effect
    /// of the boot ROM's final header-checksum comparison rather than anything
    /// meaningful.
    pub fn post_boot_dmg() -> Self {
        Registers {
            a: 0x01,
            f: Flags::from_bits(0xB0),
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            pc: 0x0100,
            sp: 0xFFFE,
        }
    }

    pub fn get8(&self, reg: Reg8) -> u8 {
        match reg {
            Reg8::A => self.a,
            Reg8::B => self.b,
            Reg8::C => self.c,
            Reg8::D => self.d,
            Reg8::E => self.e,
            Reg8::H => self.h,
            Reg8::L => self.l,
        }
    }

    pub fn set8(&mut self, reg: Reg8, value: u8) {
        match reg {
            Reg8::A => self.a = value,
            Reg8::B => self.b = value,
            Reg8::C => self.c = value,
            Reg8::D => self.d = value,
            Reg8::E => self.e = value,
            Reg8::H => self.h = value,
            Reg8::L => self.l = value,
        }
    }

    pub fn get16(&self, reg: Reg16) -> u16 {
        match reg {
            Reg16::AF => u16::from_be_bytes([self.a, self.f.bits()]),
            Reg16::BC => u16::from_be_bytes([self.b, self.c]),
            Reg16::DE => u16::from_be_bytes([self.d, self.e]),
            Reg16::HL => u16::from_be_bytes([self.h, self.l]),
            Reg16::SP => self.sp,
        }
    }

    pub fn set16(&mut self, reg: Reg16, value: u16) {
        let [hi, lo] = value.to_be_bytes();
        match reg {
            // Writing AF goes through Flags::from_bits, which drops the low nibble.
            Reg16::AF => {
                self.a = hi;
                self.f = Flags::from_bits(lo);
            }
            Reg16::BC => {
                self.b = hi;
                self.c = lo;
            }
            Reg16::DE => {
                self.d = hi;
                self.e = lo;
            }
            Reg16::HL => {
                self.h = hi;
                self.l = lo;
            }
            Reg16::SP => self.sp = value,
        }
    }
}

impl fmt::Display for Registers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "A:{:02X} F:{}{}{}{} BC:{:04X} DE:{:04X} HL:{:04X} SP:{:04X} PC:{:04X}",
            self.a,
            if self.f.zero { 'Z' } else { '-' },
            if self.f.negative { 'N' } else { '-' },
            if self.f.half_carry { 'H' } else { '-' },
            if self.f.carry { 'C' } else { '-' },
            self.get16(Reg16::BC),
            self.get16(Reg16::DE),
            self.get16(Reg16::HL),
            self.sp,
            self.pc,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_are_big_endian() {
        let mut regs = Registers::default();
        regs.set16(Reg16::HL, 0x1234);
        assert_eq!(regs.h, 0x12);
        assert_eq!(regs.l, 0x34);
        assert_eq!(regs.get16(Reg16::HL), 0x1234);
    }

    #[test]
    fn low_nibble_of_f_is_always_zero() {
        let mut regs = Registers::default();
        // Simulate POP AF with every bit set.
        regs.set16(Reg16::AF, 0xFFFF);
        assert_eq!(regs.a, 0xFF);
        assert_eq!(regs.f.bits(), 0xF0);
        assert_eq!(regs.get16(Reg16::AF), 0xFFF0);
    }

    #[test]
    fn flags_round_trip() {
        for bits in 0..=0xFu8 {
            let raw = bits << 4;
            assert_eq!(Flags::from_bits(raw).bits(), raw);
        }
    }

    #[test]
    fn post_boot_state_matches_dmg() {
        let regs = Registers::post_boot_dmg();
        assert_eq!(regs.get16(Reg16::AF), 0x01B0);
        assert_eq!(regs.get16(Reg16::BC), 0x0013);
        assert_eq!(regs.get16(Reg16::DE), 0x00D8);
        assert_eq!(regs.get16(Reg16::HL), 0x014D);
        assert_eq!(regs.pc, 0x0100);
        assert_eq!(regs.sp, 0xFFFE);
    }
}
