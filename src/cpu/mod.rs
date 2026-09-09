//! The Sharp SM83 CPU core.
//!
//! # How instructions are decoded
//!
//! The naive approach is a 256-arm match, then another 256 for the `CB` prefix. But
//! the opcode map is mostly regular: the SM83 encodes operands *inside* the opcode
//! bits, so whole families collapse into a handful of arms.
//!
//! Splitting an opcode as `xxyyyzzz` (2 bits, 3 bits, 3 bits) exposes the structure:
//!
//! - `x=1` is the entire 8-bit load block: `LD r, r'` with `y` the destination and
//!   `z` the source. 64 opcodes, one arm. (`0x76` is the exception — the slot where
//!   `LD (HL), (HL)` would be is HALT instead.)
//! - `x=2` is ALU-with-register: `y` picks the operation, `z` the operand. 64 more.
//! - `x=3` with `z=6` is the same eight ALU operations against an immediate byte.
//! - In `x=0`, `z=4` and `z=5` are `INC r` / `DEC r` with `y` naming the register.
//!
//! The `r` field (`y` or `z`) numbers registers B, C, D, E, H, L, (HL), A — where
//! slot 6 is not a register but a read or write through HL. [`Operand8`] captures
//! that distinction so the executor handles both uniformly.
//!
//! # Timing
//!
//! Every memory access costs one M-cycle, and [`Cpu`] ticks the bus as it goes rather
//! than returning a total at the end. Internal delays (the extra cycle a taken branch
//! costs, say) are ticked explicitly. So cycle counting falls out of doing the work
//! in the right order instead of being a table to keep in sync.
//!
//! Real hardware overlaps the next opcode fetch with the last cycle of the current
//! instruction ("fetch/execute overlap", chapter 5 of the reference). We don't model
//! that overlap: the observable cycle *counts* come out the same, and it keeps the
//! executor readable.

pub mod alu;
pub mod registers;

use crate::bus::Bus;
use registers::{Flags, Reg8, Reg16, Registers};

/// Where an 8-bit instruction operand lives.
///
/// The 3-bit register field encodes slot 6 as "memory at HL" rather than a register,
/// so operands are either a register or an indirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operand8 {
    Reg(Reg8),
    /// The byte at the address in HL. Costs an M-cycle to touch.
    MemHl,
}

impl Operand8 {
    /// Decodes a 3-bit register field.
    fn from_bits(bits: u8) -> Self {
        match bits & 0x07 {
            0 => Operand8::Reg(Reg8::B),
            1 => Operand8::Reg(Reg8::C),
            2 => Operand8::Reg(Reg8::D),
            3 => Operand8::Reg(Reg8::E),
            4 => Operand8::Reg(Reg8::H),
            5 => Operand8::Reg(Reg8::L),
            6 => Operand8::MemHl,
            _ => Operand8::Reg(Reg8::A),
        }
    }
}

/// The branch conditions encoded in 2 bits: NZ, Z, NC, C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Condition {
    NotZero,
    Zero,
    NotCarry,
    Carry,
}

impl Condition {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => Condition::NotZero,
            1 => Condition::Zero,
            2 => Condition::NotCarry,
            _ => Condition::Carry,
        }
    }

    fn holds(self, flags: Flags) -> bool {
        match self {
            Condition::NotZero => !flags.zero,
            Condition::Zero => flags.zero,
            Condition::NotCarry => !flags.carry,
            Condition::Carry => flags.carry,
        }
    }
}

/// Whether the CPU is running or parked waiting for an interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    /// HALT: stopped until an interrupt is pending. Cheap to resume.
    Halted,
    /// STOP: deep sleep until a button is pressed. Rarely used by games.
    Stopped,
}

pub struct Cpu {
    pub regs: Registers,
    pub state: State,
    /// Interrupt Master Enable — the global "may interrupts fire?" switch, toggled
    /// by DI and EI. Not memory-mapped; it lives inside the CPU.
    pub ime: bool,
    /// EI enables interrupts only *after* the following instruction, so a common
    /// `EI; RET` idiom returns before the handler can fire. This defers it one step.
    ime_pending: bool,
}

impl Cpu {
    /// A CPU in the state the boot ROM leaves behind, ready to execute at 0x0100.
    pub fn new() -> Self {
        Cpu {
            regs: Registers::post_boot_dmg(),
            state: State::Running,
            ime: false,
            ime_pending: false,
        }
    }

    /// Executes one instruction, ticking the bus for every cycle it consumes.
    ///
    /// When halted, burns a single M-cycle instead — the caller keeps calling this
    /// and the timer keeps running, which is what eventually produces the interrupt
    /// that wakes it.
    pub fn step(&mut self, bus: &mut impl Bus) {
        // EI's effect lands after the next instruction, so latch the request now and
        // apply it once this instruction has run.
        let enabling_ime = self.ime_pending;

        match self.state {
            State::Running => {
                let opcode = self.fetch8(bus);
                self.execute(bus, opcode);
            }
            State::Halted | State::Stopped => bus.tick(),
        }

        if enabling_ime {
            self.ime = true;
            self.ime_pending = false;
        }
    }

    // --- Memory helpers. Each access is one M-cycle. ---

    fn read8(&mut self, bus: &mut impl Bus, address: u16) -> u8 {
        bus.tick();
        bus.read(address)
    }

    fn write8(&mut self, bus: &mut impl Bus, address: u16, value: u8) {
        bus.tick();
        bus.write(address, value);
    }

    /// Reads the byte at PC and advances past it.
    fn fetch8(&mut self, bus: &mut impl Bus) -> u8 {
        let value = self.read8(bus, self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        value
    }

    /// Reads a little-endian 16-bit immediate. Two M-cycles.
    fn fetch16(&mut self, bus: &mut impl Bus) -> u16 {
        let lo = self.fetch8(bus);
        let hi = self.fetch8(bus);
        u16::from_le_bytes([lo, hi])
    }

    fn read_operand(&mut self, bus: &mut impl Bus, operand: Operand8) -> u8 {
        match operand {
            Operand8::Reg(reg) => self.regs.get8(reg),
            Operand8::MemHl => {
                let address = self.regs.get16(Reg16::HL);
                self.read8(bus, address)
            }
        }
    }

    fn write_operand(&mut self, bus: &mut impl Bus, operand: Operand8, value: u8) {
        match operand {
            Operand8::Reg(reg) => self.regs.set8(reg, value),
            Operand8::MemHl => {
                let address = self.regs.get16(Reg16::HL);
                self.write8(bus, address, value);
            }
        }
    }

    fn push16(&mut self, bus: &mut impl Bus, value: u16) {
        let [hi, lo] = value.to_be_bytes();
        // The stack grows downward, and the high byte is pushed first.
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.write8(bus, self.regs.sp, hi);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.write8(bus, self.regs.sp, lo);
    }

    fn pop16(&mut self, bus: &mut impl Bus) -> u16 {
        let lo = self.read8(bus, self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        let hi = self.read8(bus, self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }

    /// Maps the 2-bit register-pair field to a register, treating slot 3 as SP.
    /// Used by most 16-bit instructions.
    fn reg16_sp(bits: u8) -> Reg16 {
        match bits & 0x03 {
            0 => Reg16::BC,
            1 => Reg16::DE,
            2 => Reg16::HL,
            _ => Reg16::SP,
        }
    }

    /// Same field, but PUSH and POP use slot 3 for AF instead of SP.
    fn reg16_af(bits: u8) -> Reg16 {
        match bits & 0x03 {
            0 => Reg16::BC,
            1 => Reg16::DE,
            2 => Reg16::HL,
            _ => Reg16::AF,
        }
    }

    fn execute(&mut self, bus: &mut impl Bus, opcode: u8) {
        // The xxyyyzzz split described in the module docs.
        let x = opcode >> 6;
        let y = (opcode >> 3) & 0x07;
        let z = opcode & 0x07;
        // Within x=0, bit 3 of y separates paired instructions (e.g. the two
        // directions of 16-bit load), and the top two bits pick the register pair.
        let p = y >> 1;
        let q = y & 1;

        match (x, z) {
            // --- x=1: LD r, r' — the whole 8-bit register-to-register block ---
            // 0x76 would be LD (HL), (HL), which is meaningless; the hardware uses
            // that slot for HALT.
            (1, _) if opcode == 0x76 => self.halt(),
            (1, _) => {
                let value = self.read_operand(bus, Operand8::from_bits(z));
                self.write_operand(bus, Operand8::from_bits(y), value);
            }

            // --- x=2: ALU op with a register operand ---
            (2, _) => {
                let value = self.read_operand(bus, Operand8::from_bits(z));
                self.alu_op(y, value);
            }

            // --- x=3, z=6: the same ALU ops against an immediate byte ---
            (3, 6) => {
                let value = self.fetch8(bus);
                self.alu_op(y, value);
            }

            // --- x=0: a grab bag, dispatched on z ---
            (0, 0) => match y {
                0 => {} // NOP
                // LD (nn), SP — the only instruction that writes SP to memory.
                1 => {
                    let address = self.fetch16(bus);
                    let sp = self.regs.sp;
                    self.write8(bus, address, sp as u8);
                    self.write8(bus, address.wrapping_add(1), (sp >> 8) as u8);
                }
                2 => self.stop(bus),
                3 => self.jump_relative(bus, None),
                _ => self.jump_relative(bus, Some(Condition::from_bits(y - 4))),
            },

            (0, 1) => {
                if q == 0 {
                    // LD rr, nn
                    let value = self.fetch16(bus);
                    self.regs.set16(Self::reg16_sp(p), value);
                } else {
                    // ADD HL, rr — one extra cycle for the 16-bit add.
                    let hl = self.regs.get16(Reg16::HL);
                    let operand = self.regs.get16(Self::reg16_sp(p));
                    let (result, flags) = alu::add16(hl, operand, self.regs.f.zero);
                    bus.tick();
                    self.regs.set16(Reg16::HL, result);
                    self.regs.f = flags;
                }
            }

            // Indirect loads through a register pair, both directions.
            (0, 2) => {
                let address = match p {
                    0 => self.regs.get16(Reg16::BC),
                    1 => self.regs.get16(Reg16::DE),
                    // LD (HL+) / LD (HL-): use HL, then step it. These exist because
                    // copying blocks of memory is so common.
                    _ => {
                        let hl = self.regs.get16(Reg16::HL);
                        let next = if p == 2 {
                            hl.wrapping_add(1)
                        } else {
                            hl.wrapping_sub(1)
                        };
                        self.regs.set16(Reg16::HL, next);
                        hl
                    }
                };
                if q == 0 {
                    let a = self.regs.a;
                    self.write8(bus, address, a);
                } else {
                    self.regs.a = self.read8(bus, address);
                }
            }

            // INC rr / DEC rr — no flags at all, since these use the IDU rather than
            // the ALU. One extra cycle.
            (0, 3) => {
                let reg = Self::reg16_sp(p);
                let value = self.regs.get16(reg);
                let result = if q == 0 {
                    value.wrapping_add(1)
                } else {
                    value.wrapping_sub(1)
                };
                bus.tick();
                self.regs.set16(reg, result);
            }

            // INC r / DEC r — 8-bit, and these *do* set flags (except carry).
            (0, 4) | (0, 5) => {
                let operand = Operand8::from_bits(y);
                let value = self.read_operand(bus, operand);
                let (result, flags) = if z == 4 {
                    alu::inc(value, self.regs.f.carry)
                } else {
                    alu::dec(value, self.regs.f.carry)
                };
                self.write_operand(bus, operand, result);
                self.regs.f = flags;
            }

            // LD r, n
            (0, 6) => {
                let value = self.fetch8(bus);
                self.write_operand(bus, Operand8::from_bits(y), value);
            }

            // Accumulator and flag operations.
            (0, 7) => match y {
                // The four rotates of A. Note these always clear Z, unlike their
                // CB-prefixed counterparts which set it from the result.
                0 => self.rotate_a(alu::rlc(self.regs.a)),
                1 => self.rotate_a(alu::rrc(self.regs.a)),
                2 => self.rotate_a(alu::rl(self.regs.a, self.regs.f.carry)),
                3 => self.rotate_a(alu::rr(self.regs.a, self.regs.f.carry)),
                4 => {
                    let (result, flags) = alu::daa(self.regs.a, self.regs.f);
                    self.regs.a = result;
                    self.regs.f = flags;
                }
                5 => {
                    // CPL: complement A, set N and H, leave Z and C alone.
                    self.regs.a = !self.regs.a;
                    self.regs.f.negative = true;
                    self.regs.f.half_carry = true;
                }
                6 => {
                    // SCF
                    self.regs.f.negative = false;
                    self.regs.f.half_carry = false;
                    self.regs.f.carry = true;
                }
                _ => {
                    // CCF
                    self.regs.f.negative = false;
                    self.regs.f.half_carry = false;
                    self.regs.f.carry = !self.regs.f.carry;
                }
            },

            // --- x=3: control flow, stack, and the high-memory loads ---
            (3, 0) => match y {
                0..=3 => self.ret(bus, Some(Condition::from_bits(y))),
                // LDH (n), A / LDH A, (n): the 0xFF00 page holds the I/O registers,
                // so a one-byte offset covers all of them.
                4 => {
                    let offset = self.fetch8(bus);
                    let a = self.regs.a;
                    self.write8(bus, 0xFF00 + offset as u16, a);
                }
                6 => {
                    let offset = self.fetch8(bus);
                    self.regs.a = self.read8(bus, 0xFF00 + offset as u16);
                }
                // ADD SP, e
                5 => {
                    let offset = self.fetch8(bus) as i8;
                    let (result, flags) = alu::add_sp_offset(self.regs.sp, offset);
                    // Two extra cycles: the 16-bit result is assembled a byte at a time.
                    bus.tick();
                    bus.tick();
                    self.regs.sp = result;
                    self.regs.f = flags;
                }
                // LD HL, SP+e — same arithmetic, one fewer cycle.
                _ => {
                    let offset = self.fetch8(bus) as i8;
                    let (result, flags) = alu::add_sp_offset(self.regs.sp, offset);
                    bus.tick();
                    self.regs.set16(Reg16::HL, result);
                    self.regs.f = flags;
                }
            },

            (3, 1) => {
                if q == 0 {
                    let value = self.pop16(bus);
                    self.regs.set16(Self::reg16_af(p), value);
                } else {
                    match p {
                        0 => self.ret(bus, None),
                        // RETI: return and re-enable interrupts immediately — no
                        // one-instruction delay, unlike EI.
                        1 => {
                            self.ret(bus, None);
                            self.ime = true;
                        }
                        2 => {
                            // JP HL. The only jump with no memory access, so no
                            // extra cycle.
                            self.regs.pc = self.regs.get16(Reg16::HL);
                        }
                        _ => {
                            // LD SP, HL
                            bus.tick();
                            self.regs.sp = self.regs.get16(Reg16::HL);
                        }
                    }
                }
            }

            (3, 2) => match y {
                0..=3 => self.jump(bus, Some(Condition::from_bits(y))),
                // LD (C), A / LD A, (C): like LDH but the offset comes from C.
                4 => {
                    let address = 0xFF00 + self.regs.c as u16;
                    let a = self.regs.a;
                    self.write8(bus, address, a);
                }
                6 => {
                    let address = 0xFF00 + self.regs.c as u16;
                    self.regs.a = self.read8(bus, address);
                }
                // LD (nn), A / LD A, (nn)
                5 => {
                    let address = self.fetch16(bus);
                    let a = self.regs.a;
                    self.write8(bus, address, a);
                }
                _ => {
                    let address = self.fetch16(bus);
                    self.regs.a = self.read8(bus, address);
                }
            },

            (3, 3) => match y {
                0 => self.jump(bus, None),
                1 => {
                    // 0xCB: the prefix that opens the second instruction page.
                    let opcode = self.fetch8(bus);
                    self.execute_cb(bus, opcode);
                }
                6 => self.ime = false,        // DI takes effect immediately.
                7 => self.ime_pending = true, // EI is deferred one instruction.
                // 0xD3, 0xDB, 0xDD, 0xE3... are not instructions at all. Real
                // hardware locks up; we treat it as a hard error since it means our
                // decoding or the program went wrong.
                _ => panic!(
                    "illegal opcode {opcode:#04X} at {:#06X}",
                    self.regs.pc.wrapping_sub(1)
                ),
            },

            (3, 4) => match y {
                0..=3 => self.call(bus, Some(Condition::from_bits(y))),
                _ => panic!(
                    "illegal opcode {opcode:#04X} at {:#06X}",
                    self.regs.pc.wrapping_sub(1)
                ),
            },

            (3, 5) => {
                if q == 0 {
                    let value = self.regs.get16(Self::reg16_af(p));
                    // PUSH costs an extra cycle over POP for the internal decrement.
                    bus.tick();
                    self.push16(bus, value);
                } else if p == 0 {
                    self.call(bus, None);
                } else {
                    panic!(
                        "illegal opcode {opcode:#04X} at {:#06X}",
                        self.regs.pc.wrapping_sub(1)
                    );
                }
            }

            // RST n: a one-byte call to a fixed low address. Interrupt handlers and
            // size-constrained code use these.
            (3, 7) => {
                bus.tick();
                let pc = self.regs.pc;
                self.push16(bus, pc);
                self.regs.pc = (y as u16) * 8;
            }

            _ => unreachable!("opcode {opcode:#04X} escaped decoding"),
        }
    }

    /// The eight ALU operations selected by the `y` field: ADD, ADC, SUB, SBC, AND,
    /// XOR, OR, CP — in that order, in both the register and immediate blocks.
    fn alu_op(&mut self, op: u8, value: u8) {
        let a = self.regs.a;
        let carry = self.regs.f.carry;
        let (result, flags) = match op {
            0 => alu::add(a, value, false),
            1 => alu::add(a, value, carry),
            2 => alu::sub(a, value, false),
            3 => alu::sub(a, value, carry),
            4 => alu::and(a, value),
            5 => alu::xor(a, value),
            6 => alu::or(a, value),
            // CP discards the result and keeps only the flags.
            _ => (a, alu::cp(a, value)),
        };
        self.regs.a = result;
        self.regs.f = flags;
    }

    /// RLCA/RRCA/RLA/RRA always clear Z, even when the result is zero. Their
    /// CB-prefixed equivalents set it normally — an inconsistency in the ISA, not a
    /// mistake here.
    fn rotate_a(&mut self, (result, flags): (u8, Flags)) {
        self.regs.a = result;
        self.regs.f = Flags {
            zero: false,
            ..flags
        };
    }

    fn jump(&mut self, bus: &mut impl Bus, condition: Option<Condition>) {
        let target = self.fetch16(bus);
        if condition.is_none_or(|c| c.holds(self.regs.f)) {
            // A taken jump costs one more cycle than a skipped one.
            bus.tick();
            self.regs.pc = target;
        }
    }

    fn jump_relative(&mut self, bus: &mut impl Bus, condition: Option<Condition>) {
        let offset = self.fetch8(bus) as i8;
        if condition.is_none_or(|c| c.holds(self.regs.f)) {
            bus.tick();
            self.regs.pc = self.regs.pc.wrapping_add(offset as i16 as u16);
        }
    }

    fn call(&mut self, bus: &mut impl Bus, condition: Option<Condition>) {
        let target = self.fetch16(bus);
        if condition.is_none_or(|c| c.holds(self.regs.f)) {
            bus.tick();
            let pc = self.regs.pc;
            self.push16(bus, pc);
            self.regs.pc = target;
        }
    }

    fn ret(&mut self, bus: &mut impl Bus, condition: Option<Condition>) {
        if let Some(condition) = condition {
            // Conditional RET spends a cycle evaluating the condition, whether or
            // not it is taken.
            bus.tick();
            if !condition.holds(self.regs.f) {
                return;
            }
        }
        self.regs.pc = self.pop16(bus);
        bus.tick();
    }

    fn halt(&mut self) {
        self.state = State::Halted;
    }

    fn stop(&mut self, bus: &mut impl Bus) {
        // STOP is a two-byte instruction: the second byte is ignored but skipped.
        self.fetch8(bus);
        self.state = State::Stopped;
    }

    /// The CB-prefixed page: rotates, shifts, and bit operations.
    ///
    /// Far more regular than the main page. `x` selects the group and, for the bit
    /// operations, `y` is the bit index.
    fn execute_cb(&mut self, bus: &mut impl Bus, opcode: u8) {
        let x = opcode >> 6;
        let y = (opcode >> 3) & 0x07;
        let operand = Operand8::from_bits(opcode);
        let value = self.read_operand(bus, operand);
        let carry = self.regs.f.carry;

        match x {
            // x=0: the eight shift and rotate operations.
            0 => {
                let (result, flags) = match y {
                    0 => alu::rlc(value),
                    1 => alu::rrc(value),
                    2 => alu::rl(value, carry),
                    3 => alu::rr(value, carry),
                    4 => alu::sla(value),
                    5 => alu::sra(value),
                    6 => alu::swap(value),
                    _ => alu::srl(value),
                };
                self.write_operand(bus, operand, result);
                self.regs.f = flags;
            }
            // BIT only tests, so there is no writeback — and so it is one cycle
            // cheaper than RES/SET when operating on (HL).
            1 => self.regs.f = alu::bit(value, y, carry),
            2 => {
                let result = alu::res(value, y);
                self.write_operand(bus, operand, result);
            }
            _ => {
                let result = alu::set(value, y);
                self.write_operand(bus, operand, result);
            }
        }
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
