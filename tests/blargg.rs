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
