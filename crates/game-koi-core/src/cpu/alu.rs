//! The 8-bit ALU operations and their flag effects.
//!
//! These are pure functions taking the current flags and returning the result plus
//! new flags, so they can be tested exhaustively without a CPU or a bus. Getting
//! these right is most of getting the CPU right: the arithmetic itself is easy, but
//! the half-carry rules are where emulators quietly go wrong.

use super::registers::Flags;

/// The half-carry (H) flag is the carry out of bit 3 into bit 4. The trick for
/// computing it: mask both operands to their low nibble, do the operation, and see
/// whether the result overflowed the nibble.
pub fn add(a: u8, b: u8, carry_in: bool) -> (u8, Flags) {
    let c = carry_in as u16;
    let result = a as u16 + b as u16 + c;
    let half = (a & 0x0F) as u16 + (b & 0x0F) as u16 + c;
    (
        result as u8,
        Flags {
            zero: result as u8 == 0,
            negative: false,
            half_carry: half > 0x0F,
            carry: result > 0xFF,
        },
    )
}

/// Subtraction sets N, and its H flag means "borrow from bit 4" — computed by
/// checking whether the low nibble of `a` is smaller than what's being taken from it.
pub fn sub(a: u8, b: u8, carry_in: bool) -> (u8, Flags) {
    let c = carry_in as i16;
    let result = a as i16 - b as i16 - c;
    let half = (a & 0x0F) as i16 - (b & 0x0F) as i16 - c;
    (
        result as u8,
        Flags {
            zero: result as u8 == 0,
            negative: true,
            half_carry: half < 0,
            carry: result < 0,
        },
    )
}

/// AND sets H unconditionally. There is no logic to it — it is simply how the
/// hardware behaves, and code does depend on it.
pub fn and(a: u8, b: u8) -> (u8, Flags) {
    let result = a & b;
    (
        result,
        Flags {
            zero: result == 0,
            negative: false,
            half_carry: true,
            carry: false,
        },
    )
}

pub fn or(a: u8, b: u8) -> (u8, Flags) {
    let result = a | b;
    (
        result,
        Flags {
            zero: result == 0,
            negative: false,
            half_carry: false,
            carry: false,
        },
    )
}

pub fn xor(a: u8, b: u8) -> (u8, Flags) {
    let result = a ^ b;
    (
        result,
        Flags {
            zero: result == 0,
            negative: false,
            half_carry: false,
            carry: false,
        },
    )
}

/// CP is SUB that throws the result away and keeps only the flags.
pub fn cp(a: u8, b: u8) -> Flags {
    sub(a, b, false).1
}

/// INC and DEC deliberately leave the carry flag alone — the one asymmetry with
/// ADD/SUB, and a common source of bugs. The caller must pass the old carry through.
pub fn inc(value: u8, carry: bool) -> (u8, Flags) {
    let result = value.wrapping_add(1);
    (
        result,
        Flags {
            zero: result == 0,
            negative: false,
            half_carry: value & 0x0F == 0x0F,
            carry,
        },
    )
}

pub fn dec(value: u8, carry: bool) -> (u8, Flags) {
    let result = value.wrapping_sub(1);
    (
        result,
        Flags {
            zero: result == 0,
            negative: true,
            half_carry: value & 0x0F == 0,
            carry,
        },
    )
}

/// 16-bit ADD HL, rr.
///
/// The hardware performs this as two 8-bit adds, low byte then high byte, so the H
/// and C flags come from the *upper* byte's operation: carry out of bit 11 and bit
/// 15 respectively. Z is left untouched.
pub fn add16(a: u16, b: u16, zero: bool) -> (u16, Flags) {
    let result = a as u32 + b as u32;
    (
        result as u16,
        Flags {
            zero,
            negative: false,
            half_carry: (a & 0x0FFF) + (b & 0x0FFF) > 0x0FFF,
            carry: result > 0xFFFF,
        },
    )
}

/// ADD SP, e and LD HL, SP+e.
///
/// The odd one. Despite being a 16-bit result, the flags are computed from an 8-bit
/// addition of SP's low byte and the operand — so H is carry out of bit 3 and C is
/// carry out of bit 7. Z and N are always cleared, even though the result can be
/// zero. The offset is signed but the flag arithmetic treats it as unsigned.
pub fn add_sp_offset(sp: u16, offset: i8) -> (u16, Flags) {
    let offset_u = offset as u8;
    (
        sp.wrapping_add(offset as i16 as u16),
        Flags {
            zero: false,
            negative: false,
            half_carry: (sp & 0x0F) + (offset_u & 0x0F) as u16 > 0x0F,
            carry: (sp & 0xFF) + offset_u as u16 > 0xFF,
        },
    )
}

/// DAA: decimal adjust the accumulator after a BCD add or subtract.
///
/// The one instruction gekkio's reference marks TODO, so this follows the documented
/// behavior instead. The idea: BCD stores one decimal digit per nibble, but the ALU
/// added in binary, so a nibble may hold 0xA-0xF or have carried. Adding 0x06 or
/// 0x60 fixes up the low and high digit respectively.
///
/// It reads the N flag to know whether to add or subtract the correction, which is
/// the only reason N exists.
pub fn daa(a: u8, flags: Flags) -> (u8, Flags) {
    let mut result = a;
    let mut carry = flags.carry;

    if flags.negative {
        // After a subtraction, only correct where the earlier op flagged a borrow.
        if flags.carry {
            result = result.wrapping_sub(0x60);
        }
        if flags.half_carry {
            result = result.wrapping_sub(0x06);
        }
    } else {
        // After an addition, also correct when a digit exceeds 9.
        if flags.carry || result > 0x99 {
            result = result.wrapping_add(0x60);
            carry = true;
        }
        if flags.half_carry || result & 0x0F > 0x09 {
            result = result.wrapping_add(0x06);
        }
    }

    (
        result,
        Flags {
            zero: result == 0,
            negative: flags.negative,
            half_carry: false,
            carry,
        },
    )
}

/// Rotate left, bit 7 wrapping into bit 0 *and* into the carry flag.
pub fn rlc(value: u8) -> (u8, Flags) {
    let carry = value & 0x80 != 0;
    let result = value.rotate_left(1);
    (result, shift_flags(result, carry))
}

pub fn rrc(value: u8) -> (u8, Flags) {
    let carry = value & 0x01 != 0;
    let result = value.rotate_right(1);
    (result, shift_flags(result, carry))
}

/// Rotate left *through* the carry: carry becomes bit 0, bit 7 becomes the new carry.
/// A 9-bit rotation, unlike RLC.
pub fn rl(value: u8, carry_in: bool) -> (u8, Flags) {
    let carry = value & 0x80 != 0;
    let result = (value << 1) | carry_in as u8;
    (result, shift_flags(result, carry))
}

pub fn rr(value: u8, carry_in: bool) -> (u8, Flags) {
    let carry = value & 0x01 != 0;
    let result = (value >> 1) | ((carry_in as u8) << 7);
    (result, shift_flags(result, carry))
}

/// Shift left, zero into bit 0.
pub fn sla(value: u8) -> (u8, Flags) {
    let carry = value & 0x80 != 0;
    let result = value << 1;
    (result, shift_flags(result, carry))
}

/// Arithmetic shift right: bit 7 is preserved, so the sign survives.
pub fn sra(value: u8) -> (u8, Flags) {
    let carry = value & 0x01 != 0;
    let result = (value >> 1) | (value & 0x80);
    (result, shift_flags(result, carry))
}

/// Logical shift right: zero into bit 7.
pub fn srl(value: u8) -> (u8, Flags) {
    let carry = value & 0x01 != 0;
    let result = value >> 1;
    (result, shift_flags(result, carry))
}

pub fn swap(value: u8) -> (u8, Flags) {
    let result = value.rotate_left(4);
    (result, shift_flags(result, false))
}

fn shift_flags(result: u8, carry: bool) -> Flags {
    Flags {
        zero: result == 0,
        negative: false,
        half_carry: false,
        carry,
    }
}

/// BIT b, r: test a bit and set Z to its complement. Carry is preserved.
pub fn bit(value: u8, index: u8, carry: bool) -> Flags {
    Flags {
        zero: value & (1 << index) == 0,
        negative: false,
        half_carry: true,
        carry,
    }
}

pub fn res(value: u8, index: u8) -> u8 {
    value & !(1 << index)
}

pub fn set(value: u8, index: u8) -> u8 {
    value | (1 << index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_sets_half_carry_at_nibble_boundary() {
        let (result, flags) = add(0x0F, 0x01, false);
        assert_eq!(result, 0x10);
        assert!(flags.half_carry);
        assert!(!flags.carry);

        // No nibble overflow, so no H.
        let (_, flags) = add(0x0E, 0x01, false);
        assert!(!flags.half_carry);
    }

    #[test]
    fn add_wraps_and_sets_carry_and_zero() {
        let (result, flags) = add(0xFF, 0x01, false);
        assert_eq!(result, 0x00);
        assert!(flags.zero && flags.carry && flags.half_carry);
        assert!(!flags.negative);
    }

    #[test]
    fn adc_carry_in_can_trigger_both_carries() {
        // 0x0F + 0x00 + 1 overflows the low nibble on the carry alone.
        let (result, flags) = add(0x0F, 0x00, true);
        assert_eq!(result, 0x10);
        assert!(flags.half_carry);

        let (result, flags) = add(0xFF, 0xFF, true);
        assert_eq!(result, 0xFF);
        assert!(flags.carry && flags.half_carry);
    }

    #[test]
    fn sub_sets_borrow_flags() {
        let (result, flags) = sub(0x10, 0x01, false);
        assert_eq!(result, 0x0F);
        assert!(flags.half_carry, "borrow out of bit 4");
        assert!(!flags.carry);
        assert!(flags.negative);

        let (result, flags) = sub(0x00, 0x01, false);
        assert_eq!(result, 0xFF);
        assert!(flags.carry && flags.half_carry);
    }

    #[test]
    fn sbc_borrow_in_is_accounted_for() {
        let (result, flags) = sub(0x00, 0x00, true);
        assert_eq!(result, 0xFF);
        assert!(flags.carry && flags.half_carry);
    }

    #[test]
    fn inc_and_dec_preserve_carry() {
        let (_, flags) = inc(0x00, true);
        assert!(flags.carry, "INC must not touch C");
        let (_, flags) = dec(0x05, true);
        assert!(flags.carry, "DEC must not touch C");

        // But they do set H at the nibble boundary.
        let (result, flags) = inc(0x0F, false);
        assert_eq!(result, 0x10);
        assert!(flags.half_carry);
        let (result, flags) = dec(0x10, false);
        assert_eq!(result, 0x0F);
        assert!(flags.half_carry && flags.negative);
    }

    #[test]
    fn and_always_sets_half_carry() {
        let (result, flags) = and(0xF0, 0x0F);
        assert_eq!(result, 0x00);
        assert!(flags.zero && flags.half_carry);
        assert!(!flags.carry && !flags.negative);
    }

    #[test]
    fn add16_flags_come_from_the_high_byte() {
        // Carry out of bit 11 sets H.
        let (result, flags) = add16(0x0FFF, 0x0001, false);
        assert_eq!(result, 0x1000);
        assert!(flags.half_carry);
        assert!(!flags.carry);

        // A carry out of bit 7 must NOT set H: the flag comes from bit 11 only.
        // 0x00FF + 0x0001 overflows the low byte but not bit 11.
        let (result, flags) = add16(0x00FF, 0x0001, false);
        assert_eq!(result, 0x0100);
        assert!(!flags.half_carry, "H comes from bit 11, not bit 7");

        // Likewise a high nibble that carries without the low byte doing so.
        let (_, flags) = add16(0x0800, 0x0800, false);
        assert!(flags.half_carry);

        // Carry out of bit 15 sets C.
        let (result, flags) = add16(0xFFFF, 0x0001, false);
        assert_eq!(result, 0x0000);
        assert!(flags.carry && flags.half_carry);

        // Z is passed through untouched, not recomputed from the result.
        let (_, flags) = add16(0x0000, 0x0000, true);
        assert!(flags.zero, "ADD HL,rr must not touch Z");
    }

    #[test]
    fn add_sp_uses_byte_sized_flags() {
        // Flags come from the low byte even though the result is 16-bit.
        let (result, flags) = add_sp_offset(0x000F, 1);
        assert_eq!(result, 0x0010);
        assert!(flags.half_carry);
        assert!(!flags.carry);

        let (result, flags) = add_sp_offset(0x00FF, 1);
        assert_eq!(result, 0x0100);
        assert!(flags.carry && flags.half_carry);

        // Z and N are always cleared, even when the result is zero.
        let (result, flags) = add_sp_offset(0xFFFF, 1);
        assert_eq!(result, 0x0000);
        assert!(!flags.zero && !flags.negative);
    }

    #[test]
    fn add_sp_handles_negative_offsets() {
        let (result, _) = add_sp_offset(0x0100, -1);
        assert_eq!(result, 0x00FF);
        // Wrapping below zero is allowed and wraps around the 16-bit space.
        let (result, _) = add_sp_offset(0x0000, -1);
        assert_eq!(result, 0xFFFF);
    }

    #[test]
    fn daa_fixes_up_bcd_addition() {
        // 0x09 + 0x01 = 0x0A in binary; BCD wants 0x10.
        let (sum, flags) = add(0x09, 0x01, false);
        assert_eq!(sum, 0x0A);
        let (adjusted, flags) = daa(sum, flags);
        assert_eq!(adjusted, 0x10);
        assert!(!flags.carry && !flags.half_carry);

        // 0x99 + 0x01 = 0x9A -> 0x00 with a decimal carry out.
        let (sum, flags) = add(0x99, 0x01, false);
        let (adjusted, flags) = daa(sum, flags);
        assert_eq!(adjusted, 0x00);
        assert!(flags.zero && flags.carry);
    }

    #[test]
    fn daa_fixes_up_bcd_subtraction() {
        // 0x10 - 0x01 = 0x0F in binary; BCD wants 0x09.
        let (diff, flags) = sub(0x10, 0x01, false);
        assert_eq!(diff, 0x0F);
        let (adjusted, flags) = daa(diff, flags);
        assert_eq!(adjusted, 0x09);
        assert!(flags.negative, "DAA preserves N");
        assert!(!flags.carry);
    }

    #[test]
    fn daa_always_clears_half_carry() {
        for a in 0..=0xFFu8 {
            for &n in &[false, true] {
                for &h in &[false, true] {
                    for &c in &[false, true] {
                        let flags = Flags {
                            zero: false,
                            negative: n,
                            half_carry: h,
                            carry: c,
                        };
                        let (_, out) = daa(a, flags);
                        assert!(!out.half_carry);
                        assert_eq!(out.negative, n);
                    }
                }
            }
        }
    }

    #[test]
    fn rotates_through_carry_are_nine_bit() {
        // RL: carry-in becomes bit 0, bit 7 becomes the new carry.
        let (result, flags) = rl(0x80, false);
        assert_eq!(result, 0x00);
        assert!(flags.carry && flags.zero);

        let (result, flags) = rl(0x00, true);
        assert_eq!(result, 0x01);
        assert!(!flags.carry);

        // RLC wraps bit 7 straight back into bit 0 instead.
        let (result, flags) = rlc(0x80);
        assert_eq!(result, 0x01);
        assert!(flags.carry);
    }

    #[test]
    fn sra_preserves_the_sign_bit() {
        let (result, flags) = sra(0x80);
        assert_eq!(result, 0xC0, "bit 7 is copied, not cleared");
        assert!(!flags.carry);

        // SRL zeroes it instead.
        let (result, _) = srl(0x80);
        assert_eq!(result, 0x40);

        let (result, flags) = sra(0x01);
        assert_eq!(result, 0x00);
        assert!(flags.carry && flags.zero);
    }

    #[test]
    fn swap_exchanges_nibbles() {
        let (result, flags) = swap(0xAB);
        assert_eq!(result, 0xBA);
        assert!(!flags.carry && !flags.zero);
        let (_, flags) = swap(0x00);
        assert!(flags.zero);
    }

    #[test]
    fn bit_sets_zero_to_the_complement() {
        let flags = bit(0b0000_1000, 3, false);
        assert!(!flags.zero, "bit is set, so Z is clear");
        assert!(flags.half_carry);

        let flags = bit(0b0000_1000, 2, true);
        assert!(flags.zero);
        assert!(flags.carry, "BIT preserves C");
    }

    #[test]
    fn res_and_set_touch_one_bit() {
        assert_eq!(res(0xFF, 0), 0xFE);
        assert_eq!(res(0xFF, 7), 0x7F);
        assert_eq!(set(0x00, 0), 0x01);
        assert_eq!(set(0x00, 7), 0x80);
    }

    /// Cross-check: SUB and CP must agree on flags for every input pair.
    #[test]
    fn cp_matches_sub_flags() {
        for a in (0..=0xFFu8).step_by(17) {
            for b in (0..=0xFFu8).step_by(17) {
                assert_eq!(cp(a, b), sub(a, b, false).1, "CP {a:#04X} {b:#04X}");
            }
        }
    }

    /// Independent oracle: DAA must make BCD arithmetic agree with decimal
    /// arithmetic. For every pair of valid BCD values, ADD then DAA should give the
    /// BCD encoding of the decimal sum, and likewise for SUB.
    #[test]
    fn daa_matches_decimal_arithmetic() {
        let bcd = |n: u8| ((n / 10) << 4) | (n % 10);

        for x in 0..100u8 {
            for y in 0..100u8 {
                // Addition.
                let (raw, flags) = add(bcd(x), bcd(y), false);
                let (got, gf) = daa(raw, flags);
                let expected = (x + y) % 100;
                assert_eq!(got, bcd(expected), "ADD {x}+{y}");
                assert_eq!(gf.carry, x + y >= 100, "ADD carry {x}+{y}");
                assert_eq!(gf.zero, got == 0, "ADD zero {x}+{y}");

                // Subtraction.
                let (raw, flags) = sub(bcd(x), bcd(y), false);
                let (got, gf) = daa(raw, flags);
                let expected = (x as i8 - y as i8).rem_euclid(100) as u8;
                assert_eq!(got, bcd(expected), "SUB {x}-{y}");
                assert_eq!(gf.carry, x < y, "SUB carry {x}-{y}");
            }
        }
    }
}
