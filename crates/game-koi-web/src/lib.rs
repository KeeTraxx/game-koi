//! The browser frontend's half of the seam.
//!
//! This crate is to a web page what `game-koi-desktop` is to a window: it owns the
//! host side and nothing else. The emulated machine is `game-koi-core`, unchanged and
//! unaware it is running in a browser.
//!
//! # What JavaScript is expected to do
//!
//! The division of labour is deliberate. This side runs the machine and hands back
//! raw bytes; the page owns presentation and timing:
//!
//! - Call [`Emulator::new`] with the ROM bytes.
//! - Call [`Emulator::run_frame`] once per frame you want emulated.
//! - Read [`Emulator::frame_ptr`] into a `Uint8ClampedArray` and blit it to a canvas.
//! - Drain [`Emulator::take_samples`] into an `AudioWorklet`.
//!
//! Pacing is not done here, and that is the important difference from the desktop
//! build. `game-koi-desktop` sleeps until the next 16.74 ms deadline, which a browser
//! main thread must never do. The page should let one clock drive the loop — the
//! audio clock is the honest choice, since a dry sound buffer is far more audible
//! than a repeated frame — and call `run_frame` to match.

use game_koi_core::cartridge::Cartridge;
use game_koi_core::cpu::Cpu;
use game_koi_core::joypad::Button;
use game_koi_core::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use game_koi_core::testbus::TestBus;
use wasm_bindgen::prelude::*;

/// The four shades a DMG can show, as RGBA. The greenish cast is the LCD's, not the
/// palette's — the hardware only ever names four levels of "how dark".
const PALETTE: [[u8; 4]; 4] = [
    [0xE0, 0xF8, 0xD0, 0xFF],
    [0x88, 0xC0, 0x70, 0xFF],
    [0x34, 0x68, 0x56, 0xFF],
    [0x08, 0x18, 0x20, 0xFF],
];

/// A running Game Boy, owned by JavaScript.
#[wasm_bindgen]
pub struct Emulator {
    cpu: Cpu,
    bus: TestBus,
    /// The framebuffer expanded to RGBA, kept here so the canvas can read it straight
    /// out of wasm memory rather than copying a fresh array across the boundary every
    /// frame.
    rgba: Vec<u8>,
    samples: Vec<(f32, f32)>,
    /// Interleaved left/right, handed to the audio side as one flat array.
    interleaved: Vec<f32>,
}

#[wasm_bindgen]
impl Emulator {
    /// Builds a machine from a ROM image.
    ///
    /// Fails if the header will not parse or the cartridge uses a mapper the core does
    /// not implement — the same typed failure the desktop build gets, flattened to a
    /// string because that is what crosses into JavaScript.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: Vec<u8>, sample_rate: u32) -> Result<Emulator, JsError> {
        // The path is only used for error messages and save-file naming, neither of
        // which applies here, so a placeholder is honest.
        let cart = Cartridge::from_bytes(rom, "rom.gb")
            .map_err(|e| JsError::new(&format!("could not load ROM: {e}")))?;
        let mut bus =
            TestBus::new(&cart).map_err(|e| JsError::new(&format!("unsupported mapper: {e}")))?;
        bus.apu.set_sample_rate(sample_rate);

        Ok(Emulator {
            cpu: Cpu::new(),
            bus,
            rgba: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
            samples: Vec::new(),
            interleaved: Vec::new(),
        })
    }

    /// Runs until the PPU finishes a frame.
    ///
    /// The cycle budget is the same safety valve the desktop frontend uses: a ROM that
    /// somehow never completes a frame would otherwise wedge the browser tab.
    pub fn run_frame(&mut self) {
        let budget = self.bus.cycles + 100_000;
        while !self.bus.ppu.frame_ready && self.bus.cycles < budget {
            self.cpu.step(&mut self.bus);
        }
        self.bus.ppu.frame_ready = false;

        for (pixel, &shade) in self
            .rgba
            .chunks_exact_mut(4)
            .zip(self.bus.ppu.framebuffer().iter())
        {
            pixel.copy_from_slice(&PALETTE[(shade & 0x03) as usize]);
        }
    }

    /// Pointer to the RGBA framebuffer inside wasm memory.
    ///
    /// Paired with [`Emulator::frame_len`], this lets the page build a view over wasm
    /// memory once and reuse it. Any call that grows wasm memory can detach that view,
    /// so re-read both after loading a new ROM.
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    pub fn width() -> usize {
        SCREEN_WIDTH
    }

    pub fn height() -> usize {
        SCREEN_HEIGHT
    }

    /// Takes the audio produced since the last call, interleaved as L,R,L,R.
    pub fn take_samples(&mut self) -> Vec<f32> {
        self.samples.clear();
        self.bus.apu.drain_samples(&mut self.samples);
        self.interleaved.clear();
        for &(left, right) in &self.samples {
            self.interleaved.push(left);
            self.interleaved.push(right);
        }
        std::mem::take(&mut self.interleaved)
    }

    /// Presses a button, named as the page sees it ("a", "start", "dpad-up", ...).
    ///
    /// Unknown names are ignored rather than an error: a key the page does not map is
    /// not a failure worth unwinding across the boundary for.
    pub fn press(&mut self, button: &str) {
        if let Some(b) = parse_button(button) {
            self.bus.joypad.press(b, &mut self.bus.interrupts);
        }
    }

    pub fn release(&mut self, button: &str) {
        if let Some(b) = parse_button(button) {
            self.bus.joypad.release(b);
        }
    }
}

fn parse_button(name: &str) -> Option<Button> {
    Some(match name {
        "right" => Button::Right,
        "left" => Button::Left,
        "up" => Button::Up,
        "down" => Button::Down,
        "a" => Button::A,
        "b" => Button::B,
        "select" => Button::Select,
        "start" => Button::Start,
        _ => return None,
    })
}
