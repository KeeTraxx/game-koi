//! A minimal bus that is enough to run CPU test ROMs.
//!
//! Not the real hardware bus. It wires up the cartridge ROM, work RAM, the timer,
//! the serial port, and the interrupt registers — the subsystems that exist so far —
//! and treats everything else as plain memory. In particular there is no PPU, no
//! mapper, and no access restrictions.
//!
//! That is enough for Blargg's `cpu_instrs`, which only needs working instructions,
//! interrupts, and a serial port to report through. It is *not* enough to run a game.

use crate::bus::Bus;
use crate::cartridge::Cartridge;
use crate::interrupts::{Interrupt, InterruptState};
use crate::serial::Serial;
use crate::timer::Timer;

pub struct TestBus {
    rom: Vec<u8>,
    /// Everything not decoded to a device, including VRAM, WRAM, OAM, and HRAM.
    memory: Vec<u8>,
    pub timer: Timer,
    pub serial: Serial,
    pub interrupts: InterruptState,
    pub cycles: u64,
}

impl TestBus {
    pub fn new(cart: &Cartridge) -> Self {
        TestBus {
            rom: cart.rom().to_vec(),
            memory: vec![0; 0x1_0000],
            timer: Timer::new(),
            serial: Serial::new(),
            interrupts: InterruptState::default(),
            cycles: 0,
        }
    }
}

impl Bus for TestBus {
    fn read(&mut self, address: u16) -> u8 {
        match address {
            // No mapper: bank 1 is whatever physically follows bank 0. Correct only
            // for 32 KiB ROMs, which is all we support so far.
            0x0000..=0x7FFF => self.rom.get(address as usize).copied().unwrap_or(0xFF),
            0xFF01..=0xFF02 => self.serial.read(address),
            0xFF04..=0xFF07 => self.timer.read(address),
            0xFF0F => self.interrupts.read_if(),
            0xFFFF => self.interrupts.enabled,
            _ => self.memory[address as usize],
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            // Writes here are mapper commands on real hardware, never memory.
            0x0000..=0x7FFF => {}
            0xFF01..=0xFF02 => self.serial.write(address, value),
            0xFF04..=0xFF07 => self.timer.write(address, value),
            0xFF0F => self.interrupts.write_if(value),
            0xFFFF => self.interrupts.enabled = value,
            _ => self.memory[address as usize] = value,
        }
    }

    fn tick(&mut self) {
        self.cycles += 1;
        self.timer.tick(&mut self.interrupts);
        self.serial.tick(&mut self.interrupts);
    }

    fn pending_interrupt(&self) -> Option<Interrupt> {
        self.interrupts.pending()
    }

    fn acknowledge_interrupt(&mut self, interrupt: Interrupt) {
        self.interrupts.acknowledge(interrupt);
    }
}
