mod apu;
mod bus;
mod cartridge;
mod cpu;
mod frontend;
mod interrupts;
mod joypad;
mod ppu;
mod save;
mod serial;
mod testbus;
mod timer;

use std::process::ExitCode;

use bus::Bus;
use cartridge::Cartridge;
use cpu::Cpu;
use testbus::TestBus;

const USAGE: &str = "usage: gbemu-rs <rom.gb> [--info | --trace [steps] | --test | --mooneye \
                     | --screenshot [frames] | --frames [n]]";

/// What the binary was asked to do.
///
/// Parsed up front, as one value, so that an unrecognized flag is an error. The
/// previous shape — a boolean per mode, with playing in a window as the fall-through —
/// meant a typo silently opened a window and sat there waiting to be closed.
enum Mode {
    /// Play in a window, optionally exiting after a number of frames.
    Play {
        exit_after: Option<u64>,
    },
    Info,
    Trace {
        steps: usize,
    },
    Test,
    Mooneye,
    Screenshot {
        frames: usize,
    },
}

/// Decodes the mode flag and its optional numeric argument, which means something
/// different in each mode. `Err` carries the flag we did not recognize.
fn parse_mode(flag: Option<&str>, count: Option<usize>) -> Result<Mode, String> {
    Ok(match flag {
        None => Mode::Play { exit_after: None },
        Some("--frames") => Mode::Play {
            exit_after: Some(count.unwrap_or(60) as u64),
        },
        Some("--info") => Mode::Info,
        Some("--trace") => Mode::Trace {
            steps: count.unwrap_or(20),
        },
        Some("--test") => Mode::Test,
        Some("--mooneye") => Mode::Mooneye,
        Some("--screenshot") => Mode::Screenshot {
            frames: count.unwrap_or(20).max(1),
        },
        Some(other) => return Err(other.to_string()),
    })
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let flag = args.next();
    let count: Option<usize> = args.next().and_then(|s| s.parse().ok());

    let mode = match parse_mode(flag.as_deref(), count) {
        Ok(mode) => mode,
        Err(unknown) => {
            eprintln!("error: unknown option {unknown}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let cart = match Cartridge::load(&path) {
        Ok(cart) => cart,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Test => run_test_rom(&cart),
        Mode::Mooneye => run_mooneye_rom(&cart),
        Mode::Screenshot { frames } => write_screenshot(&cart, frames),

        Mode::Info => {
            print!("{}", cart.header());
            ExitCode::SUCCESS
        }

        Mode::Trace { steps } => {
            print!("{}", cart.header());
            println!();
            trace_execution(&cart, steps);
            ExitCode::SUCCESS
        }

        Mode::Play { exit_after } => {
            print!("{}", cart.header());
            println!("\ncontrols: arrows = d-pad, Z = A, X = B, Enter = Start, RShift = Select");
            println!("          P = pause, Esc = quit");
            println!("  gamepad: d-pad or left stick, East = A, South = B, Start, Select");
            if let Err(err) = frontend::run(&cart, 4, exit_after) {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
    }
}

/// Builds a bus for a cartridge, reporting an unsupported mapper rather than
/// panicking. Every run mode needs this, and none of them can do anything useful
/// without it.
fn build_bus(cart: &Cartridge) -> Option<TestBus> {
    match TestBus::new(cart) {
        Ok(bus) => Some(bus),
        Err(err) => {
            eprintln!("error: {err}");
            None
        }
    }
}

/// Reads a Blargg result out of cartridge RAM, if one has been written there.
///
/// Not every Blargg suite reports over the serial port. `dmg_sound` and friends instead
/// write their result into save RAM: a status byte at 0xA000, the signature 0xDE 0xB0
/// 0x61 right after it, and the same text it would otherwise have printed, from 0xA004
/// on. A status of 0x80 means "still running", so a harness can poll it.
///
/// Reading this goes around the RAM gate deliberately — the ROM shuts the gate before
/// halting, and a save-file dumper would ignore it too.
fn blargg_sram_result(ram: &[u8]) -> Option<(u8, String)> {
    const SIGNATURE: [u8; 3] = [0xDE, 0xB0, 0x61];
    const RUNNING: u8 = 0x80;

    if ram.len() < 4 || ram[1..4] != SIGNATURE || ram[0] == RUNNING {
        return None;
    }

    let text = ram[4..]
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as char)
        .collect();
    Some((ram[0], text))
}

/// Runs a Blargg-style test ROM and reports its result.
///
/// There are two reporting conventions and this handles both, because which one a ROM
/// uses is not something you can tell by looking at it. Most write their output one
/// character at a time to SB, finishing with "Passed" or "Failed"; the sound and timing
/// suites instead leave a status byte and the text in cartridge RAM. A ROM using the
/// second convention produces no serial output at all and then spins forever, which
/// looks exactly like a hang if the harness only knows the first.
fn run_test_rom(cart: &Cartridge) -> ExitCode {
    // A generous ceiling: the whole cpu_instrs suite needs well over 100M cycles.
    const MAX_CYCLES: u64 = 300_000_000;

    let Some(mut bus) = build_bus(cart) else {
        return ExitCode::FAILURE;
    };
    let mut cpu = Cpu::new();

    let mut sram_result = None;
    while bus.cycles < MAX_CYCLES {
        cpu.step(&mut bus);

        // Checking is cheap but not free, and only needs doing occasionally.
        if bus.cycles.is_multiple_of(0x1000) {
            let text = bus.serial.output_text();
            if text.contains("Passed") || text.contains("Failed") {
                break;
            }
            sram_result = blargg_sram_result(bus.cartridge_ram());
            if sram_result.is_some() {
                break;
            }
        }
    }

    if let Some((status, text)) = sram_result {
        println!("{}", text.trim());
        // Blargg's convention: zero is a pass, anything else is the number of failures.
        return if status == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let output = bus.serial.output_text();
    let text = output.trim();
    if text.is_empty() {
        eprintln!("no result after {} cycles", bus.cycles);
        return ExitCode::FAILURE;
    }

    println!("{text}");
    if text.contains("Passed") {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs a Mooneye test ROM, which reports differently from Blargg's.
///
/// Mooneye's ROMs finish by loading the first six Fibonacci numbers into the register
/// file and executing `LD B,B`, a no-op that acts as a debugger breakpoint. So that a
/// harness without a debugger can still read the result, they also send those same six
/// bytes out the serial port — 3, 5, 8, 13, 21, 34 for a pass, and six copies of 0x42
/// for a failure. They are mostly unprintable, which is why `--test` shows a lone
/// quote mark and calls it a failure.
fn run_mooneye_rom(cart: &Cartridge) -> ExitCode {
    const PASS: [u8; 6] = [3, 5, 8, 13, 21, 34];
    const FAIL: [u8; 6] = [0x42; 6];
    // These tests are short; anything still running after this is stuck.
    const MAX_CYCLES: u64 = 100_000_000;

    let Some(mut bus) = build_bus(cart) else {
        return ExitCode::FAILURE;
    };
    let mut cpu = Cpu::new();

    while bus.cycles < MAX_CYCLES && bus.serial.output().len() < PASS.len() {
        cpu.step(&mut bus);
    }

    let signature = bus.serial.output();
    if signature == PASS {
        println!("Passed");
        ExitCode::SUCCESS
    } else if signature == FAIL {
        println!("Failed");
        ExitCode::FAILURE
    } else {
        // Neither signature: the ROM never reached its own end, so this is a hang or a
        // crash rather than a test the hardware would call wrong.
        println!(
            "Inconclusive: serial said {signature:02X?} after {} cycles",
            bus.cycles
        );
        ExitCode::FAILURE
    }
}

/// Runs the ROM and prints each instruction's register state.
///
/// Uses the same stopgap bus as `--test`, so what you see here is what the test
/// runner sees.
fn trace_execution(cart: &Cartridge, steps: usize) {
    let Some(mut bus) = build_bus(cart) else {
        return;
    };
    let mut cpu = Cpu::new();

    for _ in 0..steps {
        let pc = cpu.regs.pc;
        let opcode = bus.read(pc);
        // Registers are printed as they are *before* the instruction runs, so each
        // line shows the inputs to the opcode next to it.
        println!(
            "{pc:04X}: {opcode:02X}  {}  DIV:{:02X}",
            cpu.regs,
            bus.timer.div()
        );
        cpu.step(&mut bus);

        if cpu.state != cpu::State::Running {
            println!("     ({:?}; nothing left to trace)", cpu.state);
            break;
        }
    }
    println!("\n{} M-cycles elapsed", bus.cycles);

    let output = bus.serial.output_text();
    if !output.is_empty() {
        println!("serial: {output}");
    }
}

/// Runs the ROM for a number of frames and writes the screen to a PGM image.
///
/// PGM is chosen because it needs no encoder: a short text header followed by one
/// byte per pixel. Any image viewer opens it, and it keeps the emulator dependency-
/// free until the real frontend arrives in step 5.
fn write_screenshot(cart: &Cartridge, frames: usize) -> ExitCode {
    use std::io::Write;

    let Some(mut bus) = build_bus(cart) else {
        return ExitCode::FAILURE;
    };
    let mut cpu = Cpu::new();

    let mut drawn = 0;
    // A frame is 70224 T-cycles; allow generous slack for a ROM that stalls.
    let budget = frames as u64 * 70_224 * 4;
    while drawn < frames && bus.cycles < budget {
        cpu.step(&mut bus);
        if bus.ppu.frame_ready {
            bus.ppu.frame_ready = false;
            drawn += 1;
        }
    }

    if drawn == 0 {
        eprintln!("no frame completed after {} cycles", bus.cycles);
        return ExitCode::FAILURE;
    }

    let path = "screenshot.pgm";
    let mut file = match std::fs::File::create(path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("error: could not write {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    let _ = writeln!(file, "P2");
    let _ = writeln!(file, "{} {}", ppu::SCREEN_WIDTH, ppu::SCREEN_HEIGHT);
    let _ = writeln!(file, "3");
    for row in bus.ppu.framebuffer().chunks(ppu::SCREEN_WIDTH) {
        // Shade 0 is lightest, so invert for a viewer where 3 is white.
        let line: Vec<String> = row.iter().map(|s| (3 - s).to_string()).collect();
        let _ = writeln!(file, "{}", line.join(" "));
    }

    println!("wrote {path} after {drawn} frame(s), {} cycles", bus.cycles);
    ExitCode::SUCCESS
}
