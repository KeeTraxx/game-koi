//! A minimal bus that is enough to run CPU test ROMs.
//!
//! Not the real hardware bus. It wires up the cartridge, work RAM, the timer, the
//! serial port, the PPU, and the interrupt registers, and treats everything else as
//! plain memory. There are still no region access restrictions beyond the PPU's own.
//!
//! Cartridge ROM and RAM go through the mapper rather than being indexed directly, so
//! banked games work; everything else about the map is flat.

use crate::bus::Bus;
use crate::cartridge::Cartridge;
use crate::cartridge::mbc::{Mbc, UnsupportedMapper};
use crate::interrupts::{Interrupt, InterruptState};
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

pub struct TestBus {
    /// The cartridge's mapper. Owns the ROM image and any save RAM, and decides which
    /// bank of each is visible right now.
    mbc: Box<dyn Mbc>,
    /// Everything not decoded to a device, including VRAM, WRAM, OAM, and HRAM.
    memory: Vec<u8>,
    pub timer: Timer,
    pub serial: Serial,
    pub ppu: Ppu,
    pub joypad: Joypad,
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
    /// Wires a bus up to a cartridge.
    ///
    /// Fails if the cartridge needs a mapper we have not written; there is no sensible
    /// way to run such a ROM, and pretending otherwise produces a garbled game rather
    /// than an error message.
    pub fn new(cart: &Cartridge) -> Result<Self, UnsupportedMapper> {
        Ok(TestBus {
            mbc: cart.create_mbc()?,
            memory: vec![0; 0x1_0000],
            timer: Timer::new(),
            serial: Serial::new(),
            ppu: Ppu::new(),
            joypad: Joypad::new(),
            interrupts: InterruptState::default(),
            cycles: 0,
            dma_source: 0,
            dma_remaining: 0,
        })
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
            0x0000..=0x7FFF => self.mbc.read_rom(source),
            0x8000..=0x9FFF => self.ppu.read_vram(source),
            // Cartridge RAM is a legal DMA source, and reads it through the mapper
            // like anything else — including reading open bus if it is disabled.
            0xA000..=0xBFFF => self.mbc.read_ram(source),
            _ => self.memory[source as usize],
        };
        self.ppu.write_oam_dma(offset, value);
        self.dma_remaining -= 1;
    }
}

impl Bus for TestBus {
    fn read(&mut self, address: u16) -> u8 {
        match address {
            0x0000..=0x7FFF => self.mbc.read_rom(address),
            0x8000..=0x9FFF => self.ppu.read_vram(address),
            0xA000..=0xBFFF => self.mbc.read_ram(address),
            0xFE00..=0xFE9F => self.ppu.read_oam(address),
            0xFF00 => self.joypad.read(),
            0xFF01..=0xFF02 => self.serial.read(address),
            0xFF04..=0xFF07 => self.timer.read(address),
            0xFF0F => self.interrupts.read_if(),
            0xFF46 => self.dma_source,
            0xFF40..=0xFF4B => self.ppu.read(address),
            // The I/O addresses nothing implements: no chip drives the bus, so it
            // floats high. The four ranges are the gaps left by the arms above —
            // unassigned bytes, plus the APU at 0xFF10-0xFF3F and the CGB registers
            // from 0xFF4C up.
            //
            // This is not a detail games ignore. `cpu_instrs.gb` probes KEY1 (0xFF4D,
            // the CGB speed switch) to decide whether to switch speed. On a DMG that
            // register does not exist and reads 0xFF, so the ROM skips the switch. Back
            // this range with plain RAM instead and the probe reads back the 0x00 it
            // just wrote, the ROM concludes it is on a CGB, and it executes STOP —
            // which halts a DMG until a button is pressed, hanging the test forever.
            0xFF03 | 0xFF08..=0xFF0E | 0xFF10..=0xFF3F | 0xFF4C..=0xFF7F => 0xFF,
            0xFFFF => self.interrupts.enabled,
            _ => self.memory[address as usize],
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            // Not memory: a write here is a command to the mapper chip.
            0x0000..=0x7FFF => self.mbc.write_rom(address, value),
            0x8000..=0x9FFF => self.ppu.write_vram(address, value),
            0xA000..=0xBFFF => self.mbc.write_ram(address, value),
            0xFE00..=0xFE9F => self.ppu.write_oam(address, value),
            0xFF00 => self.joypad.write(value),
            0xFF01..=0xFF02 => self.serial.write(address, value),
            0xFF04..=0xFF07 => self.timer.write(address, value),
            0xFF0F => self.interrupts.write_if(value),
            // Writing DMA starts a 160-byte copy into OAM from the given page.
            0xFF46 => {
                self.dma_source = value;
                self.dma_remaining = 160;
            }
            0xFF40..=0xFF4B => self.ppu.write(address, value),
            // Writes to registers nothing implements go nowhere, rather than being
            // stored where a later read would find them and mistake them for a real
            // chip answering.
            0xFF03 | 0xFF08..=0xFF0E | 0xFF10..=0xFF3F | 0xFF4C..=0xFF7F => {}
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
