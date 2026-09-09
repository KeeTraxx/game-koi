//! The host frontend: a window, input devices, and frame pacing.
//!
//! Everything here is *outside* the emulated machine. The emulator core produces a
//! framebuffer and consumes button presses; this module is what turns those into
//! something a person can use. Keeping the boundary sharp is why the core stays
//! testable without a window — `--test` and `--screenshot` never touch this module.
//!
//! Input is split out by device: [`keyboard_input`] and [`game_controller`] each
//! translate their own hardware into an [`Action`], and this module applies actions to
//! the machine. Both devices are live at once — a d-pad press and an arrow key are
//! indistinguishable by the time they reach the joypad.
//!
//! # Frame pacing
//!
//! The Game Boy runs at 4194304 T-cycles per second and a frame is 70224 of them,
//! so it refreshes at about 59.73 Hz — deliberately not 60. We run the emulator
//! until the PPU signals a completed frame, then sleep for whatever is left of the
//! frame's wall-clock budget. Pacing on the emulator's own frame counter rather than
//! the host's refresh rate keeps the emulated machine running at the right speed on
//! any monitor.

mod game_controller;
mod keyboard_input;

use std::time::{Duration, Instant};

use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use crate::cartridge::Cartridge;
use crate::cpu::Cpu;
use crate::joypad::Button;
use crate::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
use crate::testbus::TestBus;
use game_controller::GameController;

/// How long one emulated frame should take on the wall clock.
///
/// 70224 T-cycles at 4194304 Hz. Using the exact figure rather than 1/60 s keeps
/// long sessions from drifting audibly once sound exists.
const FRAME_TIME: Duration = Duration::from_nanos(16_742_706);

/// The four DMG shades as RGBA, from lightest to darkest.
///
/// The classic green-tinted LCD look rather than plain grey: the real screen was a
/// reflective LCD with a distinctly olive cast, and games were designed against it.
const PALETTE: [[u8; 4]; 4] = [
    [0xE0, 0xF8, 0xD0, 0xFF],
    [0x88, 0xC0, 0x70, 0xFF],
    [0x34, 0x68, 0x56, 0xFF],
    [0x08, 0x18, 0x20, 0xFF],
];

/// What an input device is asking the frontend to do.
///
/// The common vocabulary between [`keyboard_input`] and [`game_controller`]. Devices
/// decide *what* was meant; [`App::apply`] decides what happens as a result. Presses
/// and releases are separate variants rather than a `(Button, bool)` pair because the
/// joypad's press path also has to raise an interrupt, and keeping them distinct makes
/// the match arms say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Press(Button),
    Release(Button),
    /// Host-side controls. Not buttons the DMG had, so the emulated machine never
    /// learns these happened.
    TogglePause,
    Quit,
}

/// Runs a ROM in a window until the user closes it.
pub fn run(
    cart: &Cartridge,
    scale: u32,
    exit_after: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    // Poll rather than Wait: the emulator has work to do every frame regardless of
    // whether the OS sent us an event.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        cpu: Cpu::new(),
        bus: TestBus::new(cart)?,
        gamepad: GameController::new(),
        window: None,
        pixels: None,
        scale,
        title: cart.header().title.clone(),
        next_frame: Instant::now(),
        paused: false,
        frames: 0,
        started: Instant::now(),
        exit_after,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    cpu: Cpu,
    bus: TestBus,
    /// Polled once per frame rather than event-driven; see [`game_controller`].
    gamepad: GameController,
    /// Created on `resumed` rather than up front, because some platforms refuse to
    /// make a render surface before then.
    window: Option<std::sync::Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    scale: u32,
    title: String,
    /// When the next frame is due, for pacing.
    next_frame: Instant,
    paused: bool,
    /// Frames drawn and when we started, for the `--fps` sanity check.
    frames: u64,
    started: Instant,
    /// Exit automatically after this many frames. Used to verify pacing without a
    /// human closing the window.
    exit_after: Option<u64>,
}

impl App {
    /// Carries out one action, whichever device produced it.
    ///
    /// `event_loop` is needed only to honour [`Action::Quit`].
    fn apply(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            // A press can raise the joypad interrupt, so it needs the interrupt state;
            // a release never can, since the interrupt fires on the line going low.
            Action::Press(button) => self.bus.joypad.press(button, &mut self.bus.interrupts),
            Action::Release(button) => self.bus.joypad.release(button),
            Action::TogglePause => self.paused = !self.paused,
            Action::Quit => event_loop.exit(),
        }
    }

    /// Runs the emulator until the PPU completes a frame, then draws it.
    fn run_frame(&mut self) {
        // A safety valve: if the ROM somehow never finishes a frame, give up rather
        // than hang the event loop and freeze the window.
        let budget = self.bus.cycles + 100_000;

        while !self.bus.ppu.frame_ready && self.bus.cycles < budget {
            self.cpu.step(&mut self.bus);
        }
        self.bus.ppu.frame_ready = false;

        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };
        // Expand one shade per pixel into RGBA.
        let frame = pixels.frame_mut();
        for (pixel, &shade) in frame
            .chunks_exact_mut(4)
            .zip(self.bus.ppu.framebuffer().iter())
        {
            pixel.copy_from_slice(&PALETTE[(shade & 0x03) as usize]);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let size = winit::dpi::LogicalSize::new(
            SCREEN_WIDTH as u32 * self.scale,
            SCREEN_HEIGHT as u32 * self.scale,
        );
        let attributes = Window::default_attributes()
            .with_title(format!("gbemu-rs — {}", self.title))
            .with_inner_size(size)
            .with_min_inner_size(winit::dpi::LogicalSize::new(
                SCREEN_WIDTH as u32,
                SCREEN_HEIGHT as u32,
            ));

        let window = match event_loop.create_window(attributes) {
            Ok(window) => std::sync::Arc::new(window),
            Err(err) => {
                eprintln!("error: could not create a window: {err}");
                event_loop.exit();
                return;
            }
        };

        let physical = window.inner_size();
        let surface = SurfaceTexture::new(physical.width, physical.height, window.clone());
        match Pixels::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32, surface) {
            Ok(pixels) => self.pixels = Some(pixels),
            Err(err) => {
                eprintln!("error: could not create a render surface: {err}");
                event_loop.exit();
                return;
            }
        }

        self.window = Some(window);
        self.next_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(pixels) = self.pixels.as_mut()
                    && let Err(err) = pixels.resize_surface(size.width, size.height)
                {
                    eprintln!("error: resize failed: {err}");
                    event_loop.exit();
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Physical key, not the logical one: bindings follow the position of
                // the key on the board, so a Dvorak or AZERTY layout does not move the
                // d-pad out from under the player's hand.
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };

                // The OS's auto-repeat is a typing feature and says nothing about the
                // switch: the key is simply still down, which we already know. Passing
                // repeats through would make holding P flap the pause state.
                if event.repeat {
                    return;
                }

                if let Some(action) = keyboard_input::action_for(code, event.state) {
                    self.apply(action, event_loop);
                }
            }

            WindowEvent::RedrawRequested => {
                if let Some(pixels) = self.pixels.as_ref()
                    && let Err(err) = pixels.render()
                {
                    eprintln!("error: render failed: {err}");
                    event_loop.exit();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Gamepads are outside winit's event stream, so this is where they get read.
        // Before the sleep, not after: input gathered now is what the frame we are
        // about to run will see.
        for action in self.gamepad.poll() {
            self.apply(action, event_loop);
        }

        // Sleep until the frame is due, then run it in this same callback. Sleeping
        // and returning instead would cost an extra event-loop round trip per frame,
        // which is enough to drag the rate well below the hardware's 59.73 Hz.
        let now = Instant::now();
        if now < self.next_frame {
            std::thread::sleep(self.next_frame - now);
        }

        if !self.paused {
            self.run_frame();
            self.frames += 1;

            // Start the clock after the first frame: creating the window and GPU
            // surface takes a few hundred milliseconds, and averaging that one-off
            // cost into a short run makes the frame rate look far worse than it is.
            if self.frames == 1 {
                self.started = Instant::now();
            }

            if let Some(limit) = self.exit_after
                && self.frames >= limit
            {
                let elapsed = self.started.elapsed().as_secs_f64();
                let counted = (self.frames - 1) as f64;
                println!(
                    "{counted} frames in {elapsed:.2}s = {:.2} fps (excluding startup)",
                    counted / elapsed
                );
                event_loop.exit();
                return;
            }
        }

        // Advance by exactly one frame rather than from "now", so a late frame is
        // caught up on rather than compounding into permanent drift. If we have
        // fallen more than a frame behind, resync to avoid a death spiral.
        self.next_frame += FRAME_TIME;
        let after = Instant::now();
        if self.next_frame < after {
            self.next_frame = after + FRAME_TIME;
        }

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::ElementState;
    use winit::keyboard::KeyCode;

    #[test]
    fn frame_time_matches_the_hardware_refresh_rate() {
        // 70224 T-cycles at 4194304 Hz is ~59.73 Hz, not 60.
        let seconds = FRAME_TIME.as_secs_f64();
        let hz = 1.0 / seconds;
        assert!((hz - 59.727).abs() < 0.01, "expected ~59.73 Hz, got {hz}");
    }

    #[test]
    fn keyboard_and_gamepad_agree_on_the_same_buttons() {
        // The two devices are separate modules; this is the seam that keeps them
        // interchangeable — a gamepad press must be the same Action as a key press.
        assert_eq!(
            keyboard_input::action_for(KeyCode::ArrowUp, ElementState::Pressed),
            Some(Action::Press(Button::Up))
        );
    }

    #[test]
    fn palette_is_ordered_light_to_dark() {
        // Shade 0 is the lightest on hardware; check the brightness decreases.
        let luma = |c: [u8; 4]| c[0] as u32 + c[1] as u32 + c[2] as u32;
        for pair in PALETTE.windows(2) {
            assert!(
                luma(pair[0]) > luma(pair[1]),
                "shades must darken as the index rises"
            );
        }
    }
}
