//! Runs Blargg's test ROMs against the emulator.
//!
//! These are the real validation: they execute on the emulator itself and report
//! pass/fail through the serial port, so they check the CPU against hardware
//! behavior rather than against my understanding of it.
//!
//! The ROMs are not committed (see .gitignore). Fetch them into `test-roms/` from
//! <https://github.com/retrio/gb-test-roms>; without them these tests skip rather
//! than fail, so a fresh clone still gets a green `cargo test`.

use std::path::Path;
use std::process::Command;

/// Runs one ROM through the `--test` mode of the binary and returns its output.
///
/// Shelling out to the built binary keeps this integration test honest — it
/// exercises the same path a user does — and sidesteps needing the crate's internals
/// to be a library, which it is not yet.
fn run_rom(rom: &Path) -> Option<String> {
    if !rom.exists() {
        return None;
    }

    let binary = env!("CARGO_BIN_EXE_gbemu-rs");
    let output = Command::new(binary)
        .arg(rom)
        .arg("--test")
        .output()
        .expect("failed to run the emulator");

    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn check(name: &str) {
    let rom = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-roms/individual")
        .join(format!("{name}.gb"));

    let Some(output) = run_rom(&rom) else {
        eprintln!("skipping {name}: {} not found", rom.display());
        return;
    };

    assert!(
        output.contains("Passed"),
        "{name} did not pass:\n{}",
        output.trim()
    );
}

#[test]
fn special() {
    check("01-special");
}

#[test]
fn interrupts() {
    check("02-interrupts");
}

#[test]
fn op_sp_hl() {
    check("03-op sp,hl");
}

#[test]
fn op_r_imm() {
    check("04-op r,imm");
}

#[test]
fn op_rp() {
    check("05-op rp");
}

#[test]
fn ld_r_r() {
    check("06-ld r,r");
}

#[test]
fn jumps_and_calls() {
    check("07-jr,jp,call,ret,rst");
}

#[test]
fn misc_instrs() {
    check("08-misc instrs");
}

#[test]
fn op_r_r() {
    check("09-op r,r");
}

#[test]
fn bit_ops() {
    check("10-bit ops");
}

#[test]
fn op_a_hl() {
    check("11-op a,(hl)");
}

/// The combined ROM, which is the same 11 tests packed into a 64 KiB MBC1 cartridge.
///
/// The individual ROMs are all 32 KiB and need no mapper, so this is the one that
/// actually exercises bank switching: its runner switches banks to reach each test.
#[test]
fn cpu_instrs_combined() {
    let rom = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-roms/cpu_instrs.gb");
    let Some(output) = run_rom(&rom) else {
        eprintln!("skipping cpu_instrs: ROM not found");
        return;
    };
    assert!(
        output.contains("Passed"),
        "cpu_instrs did not pass:\n{}",
        output.trim()
    );
}

/// Runs one of the `dmg_sound` ROMs.
///
/// These report through cartridge RAM rather than the serial port; `--test` handles
/// both conventions, so this looks the same as any other check from here.
fn check_sound(name: &str) {
    let rom = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-roms/dmg_sound")
        .join(format!("{name}.gb"));

    let Some(output) = run_rom(&rom) else {
        eprintln!("skipping {name}: {} not found", rom.display());
        return;
    };

    assert!(
        output.contains("Passed"),
        "{name} did not pass:\n{}",
        output.trim()
    );
}

/// Every APU register's read-back mask, byte by byte.
#[test]
fn sound_registers() {
    check_sound("01-registers");
}

/// The length counters, at 256 Hz off the DIV-APU sequencer.
#[test]
fn sound_len_ctr() {
    check_sound("02-len ctr");
}

/// What a trigger does, including the extra length clock that depends on which half of
/// the sequencer period the write lands in.
#[test]
fn sound_trigger() {
    check_sound("03-trigger");
}

/// CH1's period sweep.
#[test]
fn sound_sweep() {
    check_sound("04-sweep");
}

/// The sweep's finer points: when the shadow register is reloaded and when the pace is
/// re-read.
#[test]
fn sound_sweep_details() {
    check_sound("05-sweep details");
}

/// The overflow check that runs immediately on trigger, before a note is heard.
#[test]
fn sound_overflow_on_trigger() {
    check_sound("06-overflow on trigger");
}

/// That the sequencer's timing stays tied to DIV across an APU power cycle, while its
/// step count restarts.
#[test]
fn sound_len_sweep_period_sync() {
    check_sound("07-len sweep period sync");
}

/// Length counters across a power cycle — they survive it on a DMG.
#[test]
fn sound_len_ctr_during_power() {
    check_sound("08-len ctr during power");
}

/// The registers a power cycle clears, and the length loads it does not.
#[test]
fn sound_regs_after_power() {
    check_sound("11-regs after power");
}

// Not run: `09-wave read while on`, `10-wave trigger while on`, and `12-wave write
// while on`. All three turn on DMG wave RAM access timing — the CPU can only reach
// wave RAM on the exact cycle CH3 reads a sample, and CH3's read position decides
// which byte it gets. Expressing that needs the CPU's memory access to be placed
// *within* an M-cycle relative to the APU's, and this bus ticks whole M-cycles with
// reads outside them. See the note in `src/apu/wave.rs`.

/// Checks instruction cycle counts, which the ROM measures against the timer — so
/// this exercises the CPU and timer together.
#[test]
fn instr_timing() {
    let rom = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-roms/instr_timing.gb");
    let Some(output) = run_rom(&rom) else {
        eprintln!("skipping instr_timing: ROM not found");
        return;
    };
    assert!(
        output.contains("Passed"),
        "instr_timing did not pass:\n{}",
        output.trim()
    );
}
