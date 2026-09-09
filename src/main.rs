mod bus;
mod cartridge;
mod cpu;
mod interrupts;
mod ppu;
mod serial;
mod testbus;
mod timer;

use std::process::ExitCode;

use bus::Bus;
use cartridge::Cartridge;
use cpu::Cpu;
use testbus::TestBus;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: gbemu-rs <rom.gb> [--trace [steps] | --test | --screenshot [frames]]");
        return ExitCode::FAILURE;
    };
    let mode = args.next();
    let trace = mode.as_deref() == Some("--trace");
    let test = mode.as_deref() == Some("--test");
    let screenshot = mode.as_deref() == Some("--screenshot");
    let steps: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);

    let cart = match Cartridge::load(&path) {
        Ok(cart) => cart,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if test {
        return run_test_rom(&cart);
    }

    if screenshot {
        // `steps` doubles as the frame count here.
        return write_screenshot(&cart, steps.max(1));
    }

    print!("{}", cart.header());

    if trace {
        println!();
        trace_execution(&cart, steps);
    }

    ExitCode::SUCCESS
}

/// Runs a Blargg-style test ROM and reports what it printed to the serial port.
///
/// These ROMs write their result one character at a time to SB, so collecting those
/// bytes turns the ROM into a pass/fail signal. They finish by writing "Passed" or
/// "Failed", then spin forever, so we stop once the text says either.
fn run_test_rom(cart: &Cartridge) -> ExitCode {
    // A generous ceiling: the whole cpu_instrs suite needs well over 100M cycles.
    const MAX_CYCLES: u64 = 300_000_000;

    let mut bus = TestBus::new(cart);
    let mut cpu = Cpu::new();

    while bus.cycles < MAX_CYCLES {
        cpu.step(&mut bus);

        // Checking the tail is cheap and only needs doing occasionally.
        if bus.cycles.is_multiple_of(0x1000) {
            let text = bus.serial.output_text();
            if text.contains("Passed") || text.contains("Failed") {
                break;
            }
        }
    }

    let output = bus.serial.output_text();
    let text = output.trim();
    if text.is_empty() {
        eprintln!("no serial output after {} cycles", bus.cycles);
        return ExitCode::FAILURE;
    }

    println!("{text}");
    if text.contains("Passed") {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs the ROM and prints each instruction's register state.
///
/// Uses the same stopgap bus as `--test`, so what you see here is what the test
/// runner sees.
fn trace_execution(cart: &Cartridge, steps: usize) {
    let mut bus = TestBus::new(cart);
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

    let mut bus = TestBus::new(cart);
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
