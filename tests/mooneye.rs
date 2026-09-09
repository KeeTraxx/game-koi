//! Runs Gekkio's Mooneye test ROMs against the emulator.
//!
//! These are stricter than Blargg's and much more specific: each ROM targets one
//! documented hardware behavior and fails on anything else. The MBC1 set is the
//! reference for mapper behavior — it checks which register bits are actually wired,
//! how bank numbers wrap on undersized chips, and that the unreachable banks really
//! are unreachable.
//!
//! Unlike Blargg's, these do not print text. They finish by loading the first six
//! Fibonacci numbers into the register file and executing `LD B,B`, and send those
//! same six bytes out the serial port so a harness without a debugger can read the
//! result. The binary's `--mooneye` mode does that comparison and prints
//! "Passed"/"Failed".
//!
//! The ROMs are not committed. Fetch a build from
//! <https://gekkio.fi/files/mooneye-test-suite/> and put the `emulator-only/mbc1`
//! directory at `test-roms/mooneye/mbc1`. Missing ROMs skip rather than fail, so a
//! fresh clone stays green — which also means a green run proves nothing if the
//! directory is empty.

use std::path::Path;
use std::process::Command;

fn check(group: &str, name: &str) {
    let rom = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-roms/mooneye")
        .join(group)
        .join(format!("{name}.gb"));

    if !rom.exists() {
        eprintln!("skipping {group}/{name}: {} not found", rom.display());
        return;
    }

    let binary = env!("CARGO_BIN_EXE_gbemu-rs");
    let output = Command::new(binary)
        .arg(&rom)
        .arg("--mooneye")
        .output()
        .expect("failed to run the emulator");
    let text = String::from_utf8_lossy(&output.stdout);

    assert!(
        text.contains("Passed"),
        "{group}/{name} did not pass: {}",
        text.trim()
    );
}

/// Which of BANK1's bits are wired: 5, so writing 0x1F selects bank 0x1F.
#[test]
fn mbc1_bits_bank1() {
    check("mbc1", "bits_bank1");
}

/// Which of BANK2's bits are wired: 2, and the rest of the written byte is ignored.
#[test]
fn mbc1_bits_bank2() {
    check("mbc1", "bits_bank2");
}

/// MODE is one bit, and only bit 0 of the written value reaches it.
#[test]
fn mbc1_bits_mode() {
    check("mbc1", "bits_mode");
}

/// RAM enable decodes the low nibble only, and only the value 0x0A unlocks it.
#[test]
fn mbc1_bits_ramg() {
    check("mbc1", "bits_ramg");
}

/// 8 KiB of save RAM: one bank, so BANK2 must not move it.
#[test]
fn mbc1_ram_64kb() {
    check("mbc1", "ram_64kb");
}

/// 32 KiB of save RAM: four banks, reachable only in mode 1.
#[test]
fn mbc1_ram_256kb() {
    check("mbc1", "ram_256kb");
}

// The ROM-size tests all check the same thing at different chip sizes: that a bank
// number wider than the chip's wired address lines wraps rather than reading garbage,
// and that the banks BANK1's zero-adjust makes unreachable stay unreachable.

#[test]
fn mbc1_rom_512kb() {
    check("mbc1", "rom_512kb");
}

#[test]
fn mbc1_rom_1mb() {
    check("mbc1", "rom_1Mb");
}

#[test]
fn mbc1_rom_2mb() {
    check("mbc1", "rom_2Mb");
}

#[test]
fn mbc1_rom_4mb() {
    check("mbc1", "rom_4Mb");
}

#[test]
fn mbc1_rom_8mb() {
    check("mbc1", "rom_8Mb");
}

/// The largest MBC1 cartridge: 2 MiB, needing all seven bank bits.
#[test]
fn mbc1_rom_16mb() {
    check("mbc1", "rom_16Mb");
}

// Not run: `multicart_rom_8Mb`. That ROM targets MBC1M, a variant board where BANK1 is
// only 4 bits wide so the cart splits into four independent 256 KiB games. It is not
// distinguishable from the header — emulators detect it by finding several Nintendo
// logos in the image — and only a handful of compilation cartridges use it. We do not
// implement it, so the test is left out rather than left failing.
