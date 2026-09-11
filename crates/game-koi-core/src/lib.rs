//! The emulated Game Boy, and nothing else.
//!
//! Everything here is the machine itself: the SM83 core, the bus that decodes its
//! address space, and the chips hanging off that bus. There is deliberately no I/O —
//! no window, no sound device, no filesystem — because the machine had none of those
//! either. A frontend drives this crate by stepping the CPU and reading the results
//! out; see `game-koi-desktop` for one that does it with a window, and
//! `game-koi-web` for one that does it in a browser.
//!
//! That absence is what lets this crate build for `wasm32-unknown-unknown` unchanged.
//! Keep it that way: `std::fs`, `std::time` and threads belong in a frontend, not
//! here. The one place the distinction gets subtle is MBC3's RTC, which counts
//! emulated M-cycles rather than asking the host what time it is — so it works the
//! same in a browser as on a desktop.

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod interrupts;
pub mod joypad;
pub mod ppu;
pub mod serial;
pub mod testbus;
pub mod timer;
