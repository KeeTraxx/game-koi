//! A minimal bus that is enough to run CPU test ROMs.
//!
//! Not the real hardware bus. It wires up the cartridge, work RAM, the timer, the
//! serial port, the PPU, and the interrupt registers, and treats everything else as
//! plain memory. There are still no region access restrictions beyond the PPU's own.
//!
//! Cartridge ROM and RAM go through the mapper rather than being indexed directly, so
//! banked games work; everything else about the map is flat.

use crate::apu::Apu;
use crate::bus::Bus;
use crate::cartridge::Cartridge;
use crate::cartridge::mbc::{Mbc, Ram, UnsupportedMapper};
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
    pub apu: Apu,
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
            apu: Apu::new(),
            joypad: Joypad::new(),
            interrupts: InterruptState::default(),
            cycles: 0,
            dma_source: 0,
            dma_remaining: 0,
        })
    }

    /// The cartridge's save RAM, for a harness reading a test ROM's result out of it.
    pub fn cartridge_ram(&self) -> &[u8] {
        self.mbc.ram().map_or(&[], Ram::bytes)
    }

    /// The RAM chip itself, for loading and storing a save file. Bypasses the gate and
    /// the bank registers, neither of which concerns a save.
    pub fn cartridge_ram_mut(&mut self) -> Option<&mut Ram> {
        self.mbc.ram_mut()
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
            0xFF10..=0xFF3F => self.apu.read(address),
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
            0xFF03 | 0xFF08..=0xFF0E | 0xFF4C..=0xFF7F => 0xFF,
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
            0xFF10..=0xFF3F => self.apu.write(address, value),
            // Writing DMA starts a 160-byte copy into OAM from the given page.
            0xFF46 => {
                self.dma_source = value;
                self.dma_remaining = 160;
            }
            0xFF40..=0xFF4B => self.ppu.write(address, value),
            // Writes to registers nothing implements go nowhere, rather than being
            // stored where a later read would find them and mistake them for a real
            // chip answering.
            0xFF03 | 0xFF08..=0xFF0E | 0xFF4C..=0xFF7F => {}
            0xFFFF => self.interrupts.enabled = value,
            _ => self.memory[address as usize] = value,
        }
    }

    fn tick(&mut self) {
        self.cycles += 1;
        // The cartridge is on the same clock as everything else — MBC3's RTC is the
        // only mapper that does anything with it.
        self.mbc.tick();
        self.timer.tick(&mut self.interrupts);
        // After the timer, so the APU sees this cycle's DIV: its 512 Hz sequencer is
        // clocked by DIV bit 4 falling, not by a clock of its own.
        self.apu.tick(self.timer.div());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::header;

    /// The smallest cartridge the loader will accept: 32 KiB, no mapper, valid logo.
    fn stub_cartridge() -> Cartridge {
        let mut rom = vec![0u8; 32 * 1024];
        header::write_logo_at(&mut rom, 0);
        let checksum = rom[0x0134..0x014D]
            .iter()
            .fold(0u8, |acc, &byte| acc.wrapping_sub(byte).wrapping_sub(1));
        rom[0x014D] = checksum;
        Cartridge::from_bytes(rom, "stub.gb").expect("stub cartridge should parse")
    }

    /// The APU answers on 0xFF10-0xFF3F, and its registers are not plain memory.
    ///
    /// Worth testing at this level rather than only inside the APU: the bus used to
    /// treat this whole range as unimplemented I/O reading 0xFF, and a routing mistake
    /// here would leave every channel silent with all the APU's own tests still green.
    #[test]
    fn the_apu_is_wired_to_its_registers() {
        let mut bus = TestBus::new(&stub_cartridge()).expect("no mapper needed");

        // Powered off, NR52 reports so and writes elsewhere do nothing.
        assert_eq!(bus.read(0xFF26) & 0x80, 0);
        bus.write(0xFF12, 0xF0);
        assert_eq!(bus.read(0xFF12), 0x00, "ignored while powered down");

        bus.write(0xFF26, 0x80);
        bus.write(0xFF12, 0xF0); // CH1 volume 15, DAC on
        bus.write(0xFF14, 0x80); // trigger
        assert_eq!(bus.read(0xFF26) & 0x01, 1, "CH1 is playing");

        // Wave RAM is memory, but reached through the APU rather than the flat array.
        bus.write(0xFF30, 0xAB);
        assert_eq!(bus.read(0xFF30), 0xAB);
    }

    /// The APU's sequencer is clocked by DIV, so it must see the timer's DIV each cycle.
    #[test]
    fn ticking_the_bus_feeds_the_apu_samples() {
        let mut bus = TestBus::new(&stub_cartridge()).expect("no mapper needed");
        bus.write(0xFF26, 0x80); // power on
        bus.write(0xFF24, 0x77); // full volume
        bus.write(0xFF25, 0xFF); // both sides
        bus.write(0xFF11, 0x80); // 50% duty
        bus.write(0xFF12, 0xF0);
        bus.write(0xFF13, 0x00);
        bus.write(0xFF14, 0x84); // period 0x400, trigger

        for _ in 0..100_000 {
            bus.tick();
        }

        let mut samples = Vec::new();
        bus.apu.drain_samples(&mut samples);
        assert!(!samples.is_empty(), "the bus must drive the APU's clock");
        let peak = samples.iter().map(|(l, _)| l.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.05, "expected audio out of the bus, peak {peak}");
    }
}
