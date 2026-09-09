mod bus;
mod cartridge;
mod cpu;

use std::process::ExitCode;

use bus::Bus;
use cartridge::Cartridge;
use cpu::Cpu;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: gbemu-rs <rom.gb> [--trace [steps]]");
        return ExitCode::FAILURE;
    };
    let trace = args.next().as_deref() == Some("--trace");
    let steps: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);

    let cart = match Cartridge::load(&path) {
        Ok(cart) => cart,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    print!("{}", cart.header());

    if trace {
        println!();
        trace_execution(&cart, steps);
    }

    ExitCode::SUCCESS
}

/// Runs the ROM and prints each instruction's register state.
///
/// A stopgap until the real bus exists: it maps cartridge ROM read-only into the low
/// 32 KiB and gives everything above it plain RAM, with no I/O behavior at all.
/// Enough to watch a game's first instructions execute and check them against a
/// disassembly, but not enough to actually run one.
fn trace_execution(cart: &Cartridge, steps: usize) {
    struct TraceBus {
        rom: Vec<u8>,
        ram: Vec<u8>,
        cycles: u64,
    }

    impl Bus for TraceBus {
        fn read(&mut self, address: u16) -> u8 {
            match address {
                0x0000..=0x7FFF => self.rom.get(address as usize).copied().unwrap_or(0xFF),
                _ => self.ram[address as usize],
            }
        }

        fn write(&mut self, address: u16, value: u8) {
            // Writes below 0x8000 are mapper commands on real hardware, not memory.
            if address >= 0x8000 {
                self.ram[address as usize] = value;
            }
        }

        fn tick(&mut self) {
            self.cycles += 1;
        }
    }

    let mut bus = TraceBus {
        rom: cart.rom().to_vec(),
        ram: vec![0; 0x1_0000],
        cycles: 0,
    };
    let mut cpu = Cpu::new();

    for _ in 0..steps {
        let pc = cpu.regs.pc;
        let opcode = bus.read(pc);
        // Registers are printed as they are *before* the instruction runs, so each
        // line shows the inputs to the opcode next to it.
        println!("{pc:04X}: {opcode:02X}  {}", cpu.regs);
        cpu.step(&mut bus);

        if cpu.state != cpu::State::Running {
            println!("     ({:?}; nothing left to trace)", cpu.state);
            break;
        }
    }
    println!("\n{} M-cycles elapsed", bus.cycles);
}
