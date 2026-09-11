//! CPU tests.
//!
//! Cycle counts asserted here are the durations documented in gekkio's reference,
//! measured in M-cycles (1 M-cycle = 4 T-cycles).

use super::*;
use crate::bus::FlatMemory;

/// Runs a program from 0x0100 and returns the CPU and memory for inspection.
fn run(program: &[u8], steps: usize) -> (Cpu, FlatMemory) {
    run_with(program, steps, |_| {})
}

/// Same, but with a chance to set up registers or memory first.
fn run_with(program: &[u8], steps: usize, setup: impl FnOnce(&mut Cpu)) -> (Cpu, FlatMemory) {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, program);
    setup(&mut cpu);
    for _ in 0..steps {
        cpu.step(&mut bus);
    }
    (cpu, bus)
}

/// Executes one instruction and reports how many M-cycles it took.
fn cycles_for(program: &[u8]) -> u64 {
    cycles_for_with(program, |_| {})
}

fn cycles_for_with(program: &[u8], setup: impl FnOnce(&mut Cpu)) -> u64 {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, program);
    setup(&mut cpu);
    cpu.step(&mut bus);
    bus.cycles
}

#[test]
fn nop_advances_pc_by_one() {
    let (cpu, _) = run(&[0x00], 1);
    assert_eq!(cpu.regs.pc, 0x0101);
    assert_eq!(cycles_for(&[0x00]), 1);
}

#[test]
fn ld_r_r_covers_the_whole_block() {
    // LD B, A with A = 0x42.
    let (cpu, _) = run_with(&[0x47], 1, |cpu| cpu.regs.a = 0x42);
    assert_eq!(cpu.regs.b, 0x42);
    assert_eq!(cycles_for(&[0x47]), 1);

    // LD D, H — checks a different pair of slots in the same decoded arm.
    let (cpu, _) = run_with(&[0x54], 1, |cpu| cpu.regs.h = 0x99);
    assert_eq!(cpu.regs.d, 0x99);
}

#[test]
fn ld_through_hl_costs_an_extra_cycle() {
    // LD B, (HL) reads memory, so 2 cycles rather than 1.
    let (cpu, _) = run_with(&[0x46], 1, |cpu| cpu.regs.set16(Reg16::HL, 0x0200));
    assert_eq!(cpu.regs.b, 0x00);
    assert_eq!(
        cycles_for_with(&[0x46], |cpu| cpu.regs.set16(Reg16::HL, 0x0200)),
        2
    );
}

#[test]
fn opcode_76_is_halt_not_a_load() {
    let (cpu, _) = run(&[0x76], 1);
    assert_eq!(cpu.state, State::Halted);
    // PC advanced past the opcode and stopped there.
    assert_eq!(cpu.regs.pc, 0x0101);
}

#[test]
fn halted_cpu_burns_cycles_without_advancing() {
    let (cpu, bus) = run(&[0x76, 0x00], 3);
    assert_eq!(cpu.state, State::Halted);
    assert_eq!(cpu.regs.pc, 0x0101, "PC must not advance while halted");
    // 1 cycle for HALT itself, then 1 per idle step.
    assert_eq!(bus.cycles, 3);
}

#[test]
fn add_a_immediate_sets_flags() {
    // ADD A, 0x01 with A = 0xFF.
    let (cpu, _) = run_with(&[0xC6, 0x01], 1, |cpu| cpu.regs.a = 0xFF);
    assert_eq!(cpu.regs.a, 0x00);
    assert!(cpu.regs.f.zero && cpu.regs.f.carry && cpu.regs.f.half_carry);
    assert_eq!(cycles_for(&[0xC6, 0x01]), 2);
}

#[test]
fn cp_leaves_the_accumulator_alone() {
    // CP 0x10 with A = 0x20: sets flags but must not modify A.
    let (cpu, _) = run_with(&[0xFE, 0x10], 1, |cpu| cpu.regs.a = 0x20);
    assert_eq!(cpu.regs.a, 0x20);
    assert!(!cpu.regs.f.zero);
    assert!(cpu.regs.f.negative);
    assert!(!cpu.regs.f.carry);

    // Equal values set Z.
    let (cpu, _) = run_with(&[0xFE, 0x20], 1, |cpu| cpu.regs.a = 0x20);
    assert!(cpu.regs.f.zero);
}

#[test]
fn inc_hl_indirect_reads_and_writes() {
    // INC (HL) — read, modify, write: 3 cycles.
    let (_, bus) = run_with(&[0x34], 1, |cpu| cpu.regs.set16(Reg16::HL, 0x0200));
    assert_eq!(bus.memory[0x0200], 0x01);
    assert_eq!(
        cycles_for_with(&[0x34], |cpu| cpu.regs.set16(Reg16::HL, 0x0200)),
        3
    );
}

#[test]
fn inc_rr_sets_no_flags() {
    // INC BC with all flags set — they must survive untouched, since 16-bit
    // increments go through the IDU rather than the ALU.
    let (cpu, _) = run_with(&[0x03], 1, |cpu| {
        cpu.regs.set16(Reg16::BC, 0x00FF);
        cpu.regs.f = Flags::from_bits(0xF0);
    });
    assert_eq!(cpu.regs.get16(Reg16::BC), 0x0100);
    assert_eq!(cpu.regs.f.bits(), 0xF0, "INC rr must not touch flags");
    assert_eq!(cycles_for(&[0x03]), 2);
}

#[test]
fn ld_hl_increment_steps_the_pointer() {
    // LD (HL+), A stores then increments.
    let (cpu, bus) = run_with(&[0x22], 1, |cpu| {
        cpu.regs.a = 0x77;
        cpu.regs.set16(Reg16::HL, 0x0200);
    });
    assert_eq!(bus.memory[0x0200], 0x77);
    assert_eq!(cpu.regs.get16(Reg16::HL), 0x0201);

    // LD A, (HL-) loads then decrements.
    let (cpu, _) = run_with(&[0x3A], 1, |cpu| cpu.regs.set16(Reg16::HL, 0x0200));
    assert_eq!(cpu.regs.get16(Reg16::HL), 0x01FF);
}

#[test]
fn ld_nn_sp_writes_both_bytes_little_endian() {
    let (_, bus) = run_with(&[0x08, 0x00, 0x02], 1, |cpu| cpu.regs.sp = 0xBEEF);
    assert_eq!(bus.memory[0x0200], 0xEF);
    assert_eq!(bus.memory[0x0201], 0xBE);
    assert_eq!(cycles_for(&[0x08, 0x00, 0x02]), 5);
}

#[test]
fn jump_absolute_takes_the_target() {
    let (cpu, _) = run(&[0xC3, 0x00, 0x02], 1);
    assert_eq!(cpu.regs.pc, 0x0200);
    assert_eq!(cycles_for(&[0xC3, 0x00, 0x02]), 4);
}

#[test]
fn conditional_jump_costs_less_when_not_taken() {
    // JP NZ, 0x0200 with Z set: falls through.
    let (cpu, _) = run_with(&[0xC2, 0x00, 0x02], 1, |cpu| cpu.regs.f.zero = true);
    assert_eq!(cpu.regs.pc, 0x0103);
    assert_eq!(
        cycles_for_with(&[0xC2, 0x00, 0x02], |cpu| cpu.regs.f.zero = true),
        3
    );

    // With Z clear it is taken, and costs one cycle more.
    let (cpu, _) = run_with(&[0xC2, 0x00, 0x02], 1, |cpu| cpu.regs.f.zero = false);
    assert_eq!(cpu.regs.pc, 0x0200);
    assert_eq!(
        cycles_for_with(&[0xC2, 0x00, 0x02], |cpu| cpu.regs.f.zero = false),
        4
    );
}

#[test]
fn relative_jump_is_signed_and_relative_to_the_next_instruction() {
    // JR +2 from 0x0100: PC is 0x0102 after the operand, so it lands at 0x0104.
    let (cpu, _) = run(&[0x18, 0x02], 1);
    assert_eq!(cpu.regs.pc, 0x0104);

    // JR -2 jumps back onto itself, the standard busy-wait idiom.
    let (cpu, _) = run(&[0x18, 0xFE], 1);
    assert_eq!(cpu.regs.pc, 0x0100);
    assert_eq!(cycles_for(&[0x18, 0xFE]), 3);
}

#[test]
fn jp_hl_is_the_cheapest_jump() {
    let (cpu, _) = run_with(&[0xE9], 1, |cpu| cpu.regs.set16(Reg16::HL, 0x0200));
    assert_eq!(cpu.regs.pc, 0x0200);
    // No memory access beyond the opcode fetch, so 1 cycle.
    assert_eq!(
        cycles_for_with(&[0xE9], |cpu| cpu.regs.set16(Reg16::HL, 0x0200)),
        1
    );
}

#[test]
fn call_pushes_the_return_address() {
    let (cpu, bus) = run(&[0xCD, 0x00, 0x02], 1);
    assert_eq!(cpu.regs.pc, 0x0200);
    assert_eq!(cpu.regs.sp, 0xFFFC, "SP dropped by two");
    // The return address 0x0103 is stored little-endian.
    assert_eq!(bus.memory[0xFFFC], 0x03);
    assert_eq!(bus.memory[0xFFFD], 0x01);
    assert_eq!(cycles_for(&[0xCD, 0x00, 0x02]), 6);
}

#[test]
fn call_and_ret_round_trip() {
    // CALL 0x0200, with a RET waiting there.
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0xCD, 0x00, 0x02]);
    bus.load(0x0200, &[0xC9]);

    cpu.step(&mut bus);
    assert_eq!(cpu.regs.pc, 0x0200);
    cpu.step(&mut bus);
    assert_eq!(cpu.regs.pc, 0x0103, "RET restored the return address");
    assert_eq!(cpu.regs.sp, 0xFFFE, "stack is balanced again");
}

#[test]
fn conditional_ret_timing_differs_by_two_cycles() {
    // RET Z, taken: 5 cycles. Not taken: 2.
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0xC8]);
    assert_eq!(cycles_for_with(&[0xC8], |cpu| cpu.regs.f.zero = true), 5);
    assert_eq!(cycles_for_with(&[0xC8], |cpu| cpu.regs.f.zero = false), 2);
}

#[test]
fn push_and_pop_round_trip_through_memory() {
    // PUSH BC then POP DE moves the value across.
    let (cpu, _) = run_with(&[0xC5, 0xD1], 2, |cpu| cpu.regs.set16(Reg16::BC, 0x1234));
    assert_eq!(cpu.regs.get16(Reg16::DE), 0x1234);
    assert_eq!(cpu.regs.sp, 0xFFFE);
    assert_eq!(
        cycles_for_with(&[0xC5], |cpu| cpu.regs.set16(Reg16::BC, 0x1234)),
        4
    );
}

#[test]
fn pop_af_discards_the_low_nibble() {
    // Push 0xFFFF through the stack into AF; F keeps only its top four bits.
    let (cpu, _) = run_with(&[0xC5, 0xF1], 2, |cpu| cpu.regs.set16(Reg16::BC, 0xFFFF));
    assert_eq!(cpu.regs.get16(Reg16::AF), 0xFFF0);
}

#[test]
fn rst_calls_a_fixed_address() {
    // RST 0x38 is opcode 0xFF.
    let (cpu, _) = run(&[0xFF], 1);
    assert_eq!(cpu.regs.pc, 0x0038);
    assert_eq!(cpu.regs.sp, 0xFFFC);
    assert_eq!(cycles_for(&[0xFF]), 4);

    // RST 0x00 is 0xC7.
    let (cpu, _) = run(&[0xC7], 1);
    assert_eq!(cpu.regs.pc, 0x0000);
}

#[test]
fn ldh_reaches_the_io_page() {
    // LDH (0x80), A writes to 0xFF80.
    let (_, bus) = run_with(&[0xE0, 0x80], 1, |cpu| cpu.regs.a = 0x5A);
    assert_eq!(bus.memory[0xFF80], 0x5A);
    assert_eq!(cycles_for(&[0xE0, 0x80]), 3);

    // LD (C), A uses C as the offset instead.
    let (_, bus) = run_with(&[0xE2], 1, |cpu| {
        cpu.regs.a = 0x5A;
        cpu.regs.c = 0x81;
    });
    assert_eq!(bus.memory[0xFF81], 0x5A);
    assert_eq!(cycles_for(&[0xE2]), 2);
}

#[test]
fn rlca_always_clears_zero() {
    // A = 0 rotates to 0, but Z stays clear — unlike the CB-prefixed RLC.
    let (cpu, _) = run_with(&[0x07], 1, |cpu| cpu.regs.a = 0x00);
    assert_eq!(cpu.regs.a, 0x00);
    assert!(!cpu.regs.f.zero, "RLCA must clear Z even on a zero result");

    // CB RLC B on zero *does* set Z.
    let (cpu, _) = run_with(&[0xCB, 0x00], 1, |cpu| cpu.regs.b = 0x00);
    assert!(cpu.regs.f.zero, "CB RLC sets Z normally");
}

#[test]
fn cb_bit_does_not_write_back() {
    // BIT 7, (HL) reads but never writes: 3 cycles, not 4.
    let (cpu, bus) = run_with(&[0xCB, 0x7E], 1, |cpu| cpu.regs.set16(Reg16::HL, 0x0200));
    assert!(cpu.regs.f.zero, "memory is zero, so bit 7 is clear");
    assert_eq!(bus.memory[0x0200], 0x00);
    assert_eq!(
        cycles_for_with(&[0xCB, 0x7E], |cpu| cpu.regs.set16(Reg16::HL, 0x0200)),
        3
    );

    // SET 7, (HL) does write back, costing one more.
    let (_, bus) = run_with(&[0xCB, 0xFE], 1, |cpu| cpu.regs.set16(Reg16::HL, 0x0200));
    assert_eq!(bus.memory[0x0200], 0x80);
    assert_eq!(
        cycles_for_with(&[0xCB, 0xFE], |cpu| cpu.regs.set16(Reg16::HL, 0x0200)),
        4
    );
}

#[test]
fn cb_operations_on_registers_take_two_cycles() {
    // SWAP A with A = 0xAB.
    let (cpu, _) = run_with(&[0xCB, 0x37], 1, |cpu| cpu.regs.a = 0xAB);
    assert_eq!(cpu.regs.a, 0xBA);
    assert_eq!(cycles_for(&[0xCB, 0x37]), 2);
}

#[test]
fn di_takes_effect_immediately_but_ei_is_deferred() {
    // EI; NOP — IME is still off after EI itself, on after the next instruction.
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0xFB, 0x00, 0x00]);

    cpu.step(&mut bus);
    assert!(
        !cpu.ime,
        "EI must not enable IME until after the next instruction"
    );
    cpu.step(&mut bus);
    assert!(cpu.ime, "IME is on once the following instruction has run");

    // DI is immediate.
    let (cpu, _) = run(&[0xF3], 1);
    assert!(!cpu.ime);
}

#[test]
fn reti_enables_interrupts_without_delay() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0xCD, 0x00, 0x02]);
    bus.load(0x0200, &[0xD9]);

    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.regs.pc, 0x0103);
    assert!(cpu.ime, "RETI enables IME immediately, unlike EI");
}

#[test]
fn add_sp_offset_instruction_timing() {
    // ADD SP, e is 4 cycles; LD HL, SP+e is 3.
    assert_eq!(cycles_for(&[0xE8, 0x01]), 4);
    assert_eq!(cycles_for(&[0xF8, 0x01]), 3);

    let (cpu, _) = run_with(&[0xF8, 0x02], 1, |cpu| cpu.regs.sp = 0x0100);
    assert_eq!(cpu.regs.get16(Reg16::HL), 0x0102);
    assert_eq!(cpu.regs.sp, 0x0100, "LD HL, SP+e must not modify SP");
}

#[test]
fn stop_consumes_its_second_byte() {
    let (cpu, _) = run(&[0x10, 0x00], 1);
    assert_eq!(cpu.state, State::Stopped);
    assert_eq!(cpu.regs.pc, 0x0102, "STOP is two bytes long");
}

#[test]
#[should_panic(expected = "illegal opcode")]
fn illegal_opcodes_are_rejected() {
    run(&[0xD3], 1);
}

/// A small program exercising several instruction families together: it sums the
/// bytes 1..=5 in a loop, then stores the result.
#[test]
fn runs_a_small_program() {
    let program = [
        0x3E, 0x00, // LD A, 0x00   accumulator
        0x06, 0x05, // LD B, 0x05   counter
        0x80, // loop: ADD A, B
        0x05, //       DEC B
        0x20, 0xFC, //       JR NZ, loop
        0xEA, 0x00, 0x02, // LD (0x0200), A
    ];
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &program);

    // Run to completion: 2 setup + 5 iterations of 3 + final store.
    for _ in 0..40 {
        cpu.step(&mut bus);
        if cpu.regs.pc >= 0x010B {
            break;
        }
    }

    // 5 + 4 + 3 + 2 + 1 = 15.
    assert_eq!(bus.memory[0x0200], 15);
    assert_eq!(cpu.regs.b, 0);
    assert!(cpu.regs.f.zero, "loop exited because B hit zero");
}

/// Every opcode must either execute or panic as illegal — never fall through
/// decoding into `unreachable!`. This is the safety net for the bit-pattern decoder:
/// it catches gaps that individual instruction tests would miss.
#[test]
fn every_opcode_is_decoded() {
    const ILLEGAL: [u8; 11] = [
        0xD3, 0xDB, 0xDD, 0xE3, 0xE4, 0xEB, 0xEC, 0xED, 0xF4, 0xFC, 0xFD,
    ];

    for opcode in 0..=0xFFu8 {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new();
        // Point HL and SP somewhere harmless so indirect accesses stay in bounds.
        cpu.regs.set16(Reg16::HL, 0x0300);
        cpu.regs.sp = 0xFFFE;
        bus.load(0x0100, &[opcode, 0x00, 0x00]);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cpu.step(&mut bus);
        }));

        if ILLEGAL.contains(&opcode) {
            assert!(result.is_err(), "opcode {opcode:#04X} should be illegal");
        } else {
            assert!(result.is_ok(), "opcode {opcode:#04X} failed to decode");
        }
    }
}

/// The same sweep for the CB page, which has no gaps at all — all 256 are valid.
#[test]
fn every_cb_opcode_is_decoded() {
    for opcode in 0..=0xFFu8 {
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new();
        cpu.regs.set16(Reg16::HL, 0x0300);
        bus.load(0x0100, &[0xCB, opcode]);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cpu.step(&mut bus);
        }));
        assert!(result.is_ok(), "CB opcode {opcode:#04X} failed to decode");
    }
}

/// Cycle counts for every opcode whose duration gekkio's reference states
/// unambiguously for a single opcode value. Extracted from the PDF rather than
/// typed by hand, so this checks the implementation against the document directly.
///
/// Conditional instructions are excluded: their documented duration covers only one
/// of the two paths, and the taken/not-taken split is asserted separately above.
#[test]
fn documented_cycle_counts_match() {
    const DOCUMENTED: &[(u8, u64)] = &[
        (0x00, 1),
        (0x02, 2),
        (0x07, 1),
        (0x08, 5),
        (0x0A, 2),
        (0x0F, 1),
        (0x12, 2),
        (0x17, 1),
        (0x18, 3),
        (0x1A, 2),
        (0x1F, 1),
        (0x22, 2),
        (0x27, 1),
        (0x2A, 2),
        (0x2F, 1),
        (0x32, 2),
        (0x34, 3),
        (0x35, 3),
        (0x36, 3),
        (0x37, 1),
        (0x3A, 2),
        (0x3F, 1),
        (0x86, 2),
        (0x8E, 2),
        (0x96, 2),
        (0x9E, 2),
        (0xA6, 2),
        (0xAE, 2),
        (0xB6, 2),
        (0xBE, 2),
        (0xC3, 4),
        (0xC6, 2),
        (0xC9, 4),
        (0xCD, 6),
        (0xCE, 2),
        (0xD6, 2),
        (0xD9, 4),
        (0xDE, 2),
        (0xE0, 3),
        (0xE2, 2),
        (0xE6, 2),
        (0xE8, 4),
        (0xE9, 1),
        (0xEA, 4),
        (0xEE, 2),
        (0xF0, 3),
        (0xF2, 2),
        (0xF3, 1),
        (0xF6, 2),
        (0xF8, 3),
        (0xF9, 2),
        (0xFA, 4),
        (0xFB, 1),
        (0xFE, 2),
    ];

    // Conditional branches have two durations; the reference lists the taken one.
    const CONDITIONAL: [u8; 12] = [
        0x20, 0x28, 0x30, 0x38, // JR cc
        0xC0, 0xC8, 0xD0, 0xD8, // RET cc
        0xC2, 0xCA, 0xD2, 0xDA, // JP cc
    ];

    for &(opcode, expected) in DOCUMENTED {
        if CONDITIONAL.contains(&opcode) {
            continue;
        }
        let mut cpu = Cpu::new();
        let mut bus = FlatMemory::new();
        cpu.regs.set16(Reg16::HL, 0x0300);
        cpu.regs.sp = 0xFFFE;
        bus.load(0x0100, &[opcode, 0x00, 0x00]);
        cpu.step(&mut bus);
        assert_eq!(
            bus.cycles, expected,
            "opcode {opcode:#04X} took {} M-cycles, reference says {expected}",
            bus.cycles
        );
    }
}

// --- Interrupt dispatch ---

use crate::interrupts::Interrupt;

/// Arms an interrupt as both requested and enabled.
fn request(bus: &mut FlatMemory, interrupt: Interrupt) {
    bus.interrupts.enabled |= interrupt.mask();
    bus.interrupts.request(interrupt);
}

#[test]
fn dispatch_calls_the_handler_and_clears_ime() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x00, 0x00]);
    cpu.ime = true;
    request(&mut bus, Interrupt::Timer);

    cpu.step(&mut bus);

    assert_eq!(cpu.regs.pc, 0x0050, "jumped to the timer handler");
    assert!(!cpu.ime, "IME is cleared so the handler is not re-entered");
    assert_eq!(
        bus.interrupts.requested & Interrupt::Timer.mask(),
        0,
        "the IF bit is cleared by the CPU, not the handler"
    );
    // The return address was pushed, so RETI can get back.
    assert_eq!(cpu.regs.sp, 0xFFFC);
    assert_eq!(bus.memory[0xFFFC], 0x00);
    assert_eq!(bus.memory[0xFFFD], 0x01);
    assert_eq!(bus.cycles, 5, "dispatch takes 5 M-cycles");
}

#[test]
fn dispatch_does_not_happen_while_ime_is_off() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x00]);
    cpu.ime = false;
    request(&mut bus, Interrupt::Timer);

    cpu.step(&mut bus);
    assert_eq!(cpu.regs.pc, 0x0101, "the NOP ran instead");
    assert_ne!(bus.interrupts.requested, 0, "the request is still pending");
}

#[test]
fn dispatch_respects_priority() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    cpu.ime = true;
    request(&mut bus, Interrupt::Joypad);
    request(&mut bus, Interrupt::VBlank);

    cpu.step(&mut bus);
    assert_eq!(cpu.regs.pc, 0x0040, "VBlank outranks Joypad");
    // Joypad is untouched and will be serviced next.
    assert_ne!(bus.interrupts.requested & Interrupt::Joypad.mask(), 0);
}

#[test]
fn a_pending_interrupt_wakes_a_halted_cpu() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x76, 0x00]); // HALT; NOP
    cpu.ime = true;

    cpu.step(&mut bus);
    assert_eq!(cpu.state, State::Halted);

    // Nothing pending: it stays halted.
    cpu.step(&mut bus);
    assert_eq!(cpu.state, State::Halted);

    request(&mut bus, Interrupt::Timer);
    cpu.step(&mut bus);
    assert_eq!(cpu.state, State::Running, "the interrupt woke it");
    assert_eq!(cpu.regs.pc, 0x0050, "and was dispatched");
}

#[test]
fn halt_wakes_even_when_ime_is_off() {
    // Waking and dispatching are separate questions: IME gates only the dispatch.
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x76, 0x3C]); // HALT; INC A
    cpu.ime = false;
    cpu.regs.a = 0x00;

    cpu.step(&mut bus);
    assert_eq!(cpu.state, State::Halted);

    request(&mut bus, Interrupt::Timer);
    cpu.step(&mut bus); // The wake itself consumes this step.
    assert_eq!(cpu.state, State::Running);
    assert_eq!(cpu.regs.pc, 0x0101, "resumed inline, no handler jump");
    assert_ne!(bus.interrupts.requested, 0, "IF is untouched with IME off");

    cpu.step(&mut bus); // Now the instruction after HALT runs, exactly once.
    assert_eq!(cpu.regs.a, 1);
    assert_eq!(cpu.regs.pc, 0x0102);
}

#[test]
fn halt_bug_executes_the_next_byte_twice() {
    // IME off with an interrupt already pending: HALT does not halt, and the
    // following byte is executed twice because PC fails to advance once.
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x76, 0x3C, 0x00]); // HALT; INC A; NOP
    cpu.ime = false;
    cpu.regs.a = 0x00;
    request(&mut bus, Interrupt::Timer);

    cpu.step(&mut bus); // HALT: triggers the bug rather than halting.
    assert_eq!(cpu.state, State::Running, "the bug means it never halts");

    cpu.step(&mut bus); // INC A, but PC does not advance.
    assert_eq!(cpu.regs.a, 1);
    assert_eq!(cpu.regs.pc, 0x0101, "PC stuck on the INC");

    cpu.step(&mut bus); // The same INC A runs again.
    assert_eq!(cpu.regs.a, 2, "the byte after HALT executed twice");
    assert_eq!(cpu.regs.pc, 0x0102);
}

#[test]
fn ei_then_halt_dispatches_normally() {
    // With IME on, HALT behaves properly and no bug is triggered.
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x76, 0x3C]);
    cpu.ime = true;
    request(&mut bus, Interrupt::Timer);

    cpu.step(&mut bus);
    assert_eq!(cpu.regs.pc, 0x0050, "dispatched instead of halting");
}

#[test]
fn handler_returns_with_reti_and_restores_ime() {
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x00, 0x3C]); // NOP; INC A
    bus.load(0x0050, &[0xD9]); // handler: RETI
    cpu.ime = true;
    request(&mut bus, Interrupt::Timer);

    cpu.step(&mut bus); // dispatch
    assert_eq!(cpu.regs.pc, 0x0050);
    assert!(!cpu.ime);

    cpu.step(&mut bus); // RETI
    assert_eq!(cpu.regs.pc, 0x0100, "returned to where we were interrupted");
    assert!(cpu.ime, "RETI restored IME");
    assert_eq!(cpu.regs.sp, 0xFFFE, "stack balanced");
}

#[test]
fn dispatch_waits_until_the_current_instruction_finishes() {
    // The interrupt arrives while a multi-cycle instruction is mid-flight; it must
    // not be serviced until that instruction completes.
    let mut cpu = Cpu::new();
    let mut bus = FlatMemory::new();
    bus.load(0x0100, &[0x01, 0x34, 0x12, 0x00]); // LD BC, 0x1234; NOP
    cpu.ime = true;
    request(&mut bus, Interrupt::Timer);

    cpu.step(&mut bus);
    // The dispatch happens first, before LD BC even starts.
    assert_eq!(cpu.regs.pc, 0x0050);
    assert_eq!(cpu.regs.get16(Reg16::BC), 0x0013, "LD BC has not run yet");
}

/// End-to-end: a real timer driving a real handler through the CPU.
///
/// This is the first test where all three of step 3's pieces run together — the
/// timer overflows, sets its `IF` bit, the CPU dispatches to 0x0050, the handler
/// runs, and `RETI` returns to the interrupted loop with IME restored.
#[test]
fn timer_interrupt_drives_a_handler() {
    use crate::interrupts::InterruptState;
    use crate::timer::Timer;

    /// A bus with a working timer, unlike the inert `FlatMemory`.
    struct TimedBus {
        memory: Vec<u8>,
        timer: Timer,
        interrupts: InterruptState,
    }

    impl Bus for TimedBus {
        fn read(&mut self, address: u16) -> u8 {
            match address {
                0xFF04..=0xFF07 => self.timer.read(address),
                0xFF0F => self.interrupts.read_if(),
                0xFFFF => self.interrupts.enabled,
                _ => self.memory[address as usize],
            }
        }

        fn write(&mut self, address: u16, value: u8) {
            match address {
                0xFF04..=0xFF07 => self.timer.write(address, value),
                0xFF0F => self.interrupts.write_if(value),
                0xFFFF => self.interrupts.enabled = value,
                _ => self.memory[address as usize] = value,
            }
        }

        fn tick(&mut self) {
            self.timer.tick(&mut self.interrupts);
        }

        fn pending_interrupt(&self) -> Option<Interrupt> {
            self.interrupts.pending()
        }

        fn acknowledge_interrupt(&mut self, interrupt: Interrupt) {
            self.interrupts.acknowledge(interrupt);
        }
    }

    let mut bus = TimedBus {
        memory: vec![0; 0x1_0000],
        timer: Timer::new(),
        interrupts: InterruptState::default(),
    };

    // Main loop: enable the timer interrupt, start the fastest timer, EI, spin.
    bus.memory[0x0100..0x010D].copy_from_slice(&[
        0x3E, 0x04, // LD A, 0x04
        0xE0, 0xFF, // LDH (0xFF), A   -> IE = timer
        0x3E, 0x05, // LD A, 0x05
        0xE0, 0x07, // LDH (0x07), A   -> TAC = enabled, fastest
        0xFB, // EI
        0x00, 0x00, // NOP; NOP
        0x18, 0xFC, // JR -4
    ]);
    // Handler at the timer vector: count invocations in B, then return.
    bus.memory[0x0050..0x0052].copy_from_slice(&[0x04, 0xD9]); // INC B; RETI

    let mut cpu = Cpu::new();
    for _ in 0..4000 {
        cpu.step(&mut bus);
    }

    assert!(
        cpu.regs.b >= 3,
        "handler should have run several times, B = {}",
        cpu.regs.b
    );
    assert!(cpu.ime, "RETI left interrupts enabled");
    assert_eq!(cpu.regs.sp, 0xFFFE, "every dispatch was matched by a RETI");
    // Execution ended back in the spin loop, not stranded in the handler.
    assert!(
        (0x0109..=0x010D).contains(&cpu.regs.pc),
        "PC = {:#06X} is outside the main loop",
        cpu.regs.pc
    );
}
