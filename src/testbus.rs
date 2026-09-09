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
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

pub struct TestBus {
    rom: Vec<u8>,
    /// Everything not decoded to a device, including VRAM, WRAM, OAM, and HRAM.
    memory: Vec<u8>,
    pub timer: Timer,
    pub serial: Serial,
    pub ppu: Ppu,
    pub interrupts: InterruptState,
    pub cycles: u64,
    /// OAM DMA state: the source page and how many bytes are left to copy.
    ///
    /// The transfer moves one byte per M-cycle for 160 M-cycles, running alongside
    /// the CPU rather than stopping it. Games start one during VBlank and use the
    /// time for other work.
    dma_source: u8,
    dma_remaining: u8,
}

impl TestBus {
    pub fn new(cart: &Cartridge) -> Self {
        TestBus {
            rom: cart.rom().to_vec(),
            memory: vec![0; 0x1_0000],
            timer: Timer::new(),
            serial: Serial::new(),
            ppu: Ppu::new(),
            interrupts: InterruptState::default(),
            cycles: 0,
            dma_source: 0,
            dma_remaining: 0,
        }
    }

    /// Copies one byte of an in-progress OAM DMA transfer.
    ///
    /// Writes go through `write_oam_dma`, which ignores the PPU's mode locking —
    /// the DMA controller, not the CPU, is doing the writing.
    fn step_dma(&mut self) {
        if self.dma_remaining == 0 {
            return;
        }

        let offset = 160 - self.dma_remaining;
        let source = ((self.dma_source as u16) << 8) | offset as u16;
        let value = match source {
            0x0000..=0x7FFF => self.rom.get(source as usize).copied().unwrap_or(0xFF),
            0x8000..=0x9FFF => self.ppu.read_vram(source),
            _ => self.memory[source as usize],
        };
        self.ppu.write_oam_dma(offset, value);
        self.dma_remaining -= 1;
    }
}

impl Bus for TestBus {
    fn read(&mut self, address: u16) -> u8 {
        match address {
            // No mapper: bank 1 is whatever physically follows bank 0. Correct only
            // for 32 KiB ROMs, which is all we support so far.
            0x0000..=0x7FFF => self.rom.get(address as usize).copied().unwrap_or(0xFF),
            0x8000..=0x9FFF => self.ppu.read_vram(address),
            0xFE00..=0xFE9F => self.ppu.read_oam(address),
            0xFF01..=0xFF02 => self.serial.read(address),
            0xFF04..=0xFF07 => self.timer.read(address),
            0xFF0F => self.interrupts.read_if(),
            0xFF46 => self.dma_source,
            0xFF40..=0xFF4B => self.ppu.read(address),
            0xFFFF => self.interrupts.enabled,
            _ => self.memory[address as usize],
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            // Writes here are mapper commands on real hardware, never memory.
            0x0000..=0x7FFF => {}
            0x8000..=0x9FFF => self.ppu.write_vram(address, value),
            0xFE00..=0xFE9F => self.ppu.write_oam(address, value),
            0xFF01..=0xFF02 => self.serial.write(address, value),
            0xFF04..=0xFF07 => self.timer.write(address, value),
            0xFF0F => self.interrupts.write_if(value),
            // Writing DMA starts a 160-byte copy into OAM from the given page.
            0xFF46 => {
                self.dma_source = value;
                self.dma_remaining = 160;
            }
            0xFF40..=0xFF4B => self.ppu.write(address, value),
            0xFFFF => self.interrupts.enabled = value,
            _ => self.memory[address as usize] = value,
        }
    }

    fn tick(&mut self) {
        self.cycles += 1;
        self.timer.tick(&mut self.interrupts);
        self.serial.tick(&mut self.interrupts);
        self.ppu.tick(&mut self.interrupts);
        self.step_dma();
    }

    fn pending_interrupt(&self) -> Option<Interrupt> {
        self.interrupts.pending()
    }

    fn acknowledge_interrupt(&mut self, interrupt: Interrupt) {
        self.interrupts.acknowledge(interrupt);
    }
}
