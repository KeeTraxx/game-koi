//! The memory bus interface.
//!
//! Everything the CPU can reach goes through this trait. It is a trait rather than a
//! concrete type so the CPU can be tested against a flat 64 KiB array without
//! dragging in the cartridge, PPU, and timer — and so the real bus can grow those
//! subsystems without the CPU noticing.
//!
//! Reads and writes are deliberately `&mut self`: on real hardware a read can have
//! side effects, and there is no way to express "reading this register clears it"
//! with a shared borrow.

use crate::interrupts::Interrupt;

/// A device the CPU can read from and write to.
pub trait Bus {
    fn read(&mut self, address: u16) -> u8;
    fn write(&mut self, address: u16, value: u8);

    /// Advances the rest of the system by one M-cycle (4 T-cycles).
    ///
    /// The CPU calls this for every machine cycle it spends, including the ones where
    /// it isn't touching memory. That is what keeps the PPU and timer in step with
    /// instruction execution rather than being advanced in a lump afterwards.
    fn tick(&mut self);

    /// The highest-priority interrupt that is requested *and* enabled, if any.
    ///
    /// Interrupts flow this way — the CPU asks the bus — rather than devices calling
    /// into the CPU. A device raises an interrupt by setting a bit in `IF`, which
    /// lives on the bus, and the CPU polls it between instructions. That keeps
    /// ownership one-directional and avoids the `Rc<RefCell<...>>` tangle that a
    /// callback design would need.
    fn pending_interrupt(&self) -> Option<Interrupt>;

    /// Clears a source's `IF` bit as the CPU dispatches it.
    fn acknowledge_interrupt(&mut self, interrupt: Interrupt);
}

/// A flat 64 KiB memory with no device behavior, for testing the CPU in isolation.
#[cfg(test)]
pub struct FlatMemory {
    pub memory: Vec<u8>,
    /// Counts M-cycles so tests can assert on instruction timing.
    pub cycles: u64,
    /// Real interrupt registers, so CPU dispatch can be tested without the full bus.
    pub interrupts: crate::interrupts::InterruptState,
}

#[cfg(test)]
impl FlatMemory {
    pub fn new() -> Self {
        FlatMemory {
            memory: vec![0; 0x1_0000],
            cycles: 0,
            interrupts: crate::interrupts::InterruptState::default(),
        }
    }

    /// Loads bytes at an address, for placing a test program.
    pub fn load(&mut self, address: u16, bytes: &[u8]) -> &mut Self {
        let start = address as usize;
        self.memory[start..start + bytes.len()].copy_from_slice(bytes);
        self
    }
}

#[cfg(test)]
impl Bus for FlatMemory {
    fn read(&mut self, address: u16) -> u8 {
        self.memory[address as usize]
    }

    fn write(&mut self, address: u16, value: u8) {
        self.memory[address as usize] = value;
    }

    fn tick(&mut self) {
        self.cycles += 1;
    }

    fn pending_interrupt(&self) -> Option<Interrupt> {
        self.interrupts.pending()
    }

    fn acknowledge_interrupt(&mut self, interrupt: Interrupt) {
        self.interrupts.acknowledge(interrupt);
    }
}
