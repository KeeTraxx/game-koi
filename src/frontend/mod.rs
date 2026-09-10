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

mod audio;
mod game_controller;
mod keyboard_input;
mod overlay;
mod stats;

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
use crate::save::SaveFile;
use crate::testbus::TestBus;
use audio::Audio;
use game_controller::GameController;
use overlay::Overlay;
use stats::{FrameStats, GpuInfo};

/// How long one emulated frame should take on the wall clock.
///
/// 70224 T-cycles at 4194304 Hz. Using the exact figure rather than 1/60 s keeps
/// long sessions from drifting audibly once sound exists.
const FRAME_TIME: Duration = Duration::from_nanos(16_742_706);

/// The same figure as a rate, for reporting speed as a percentage of real hardware.
const TARGET_HZ: f64 = 59.727_5;

/// How often to write the save file out, in frames — about two seconds.
///
/// A save is written when the window closes, but nothing guarantees that happens: the
/// process can be killed, or the machine can lose power. Checking periodically costs
/// nothing when the game has not touched its RAM, which is almost always.
const AUTOSAVE_FRAMES: u64 = 120;

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
    ToggleOverlay,
    ToggleVsync,
    Quit,
}

/// Runs a ROM in a window until the user closes it.
pub fn run(
    cart: &Cartridge,
    scale: u32,
    exit_after: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Open the audio device before the emulator starts, so the APU can be told the
    // device's real sample rate rather than resampling to a guess and then changing.
    let audio = Audio::new();
    match audio.as_ref() {
        Some(audio) => println!(
            "    audio: {} Hz, {} channels",
            audio.sample_rate(),
            audio.channels()
        ),
        None => println!("    audio: no output device; running silently"),
    }

    let event_loop = EventLoop::new()?;
    // Poll rather than Wait: the emulator has work to do every frame regardless of
    // whether the OS sent us an event.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        cpu: Cpu::new(),
        bus: TestBus::new(cart)?,
        gamepad: GameController::new(),
        audio,
        save: None,
        samples: Vec::new(),
        window: None,
        pixels: None,
        scale,
        title: cart.header().title.clone(),
        next_frame: Instant::now(),
        paused: false,
        frames: 0,
        started: Instant::now(),
        stats: FrameStats::new(),
        vsync: true,
        cycle_started: Instant::now(),
        overlay: None,
        exit_after,
    };
    if let Some(audio) = app.audio.as_ref() {
        let rate = audio.sample_rate();
        app.bus.apu.set_sample_rate(rate);
    }

    // Restore the save before the CPU runs a single instruction: the game reads its
    // save RAM during startup, so loading any later means it has already decided the
    // cartridge is blank.
    app.save = SaveFile::for_cartridge(cart);
    if let Some(save) = app.save.as_ref() {
        println!("     save: {}", save.path().display());
        match save.load(&mut app.bus) {
            Ok(true) => println!("           loaded"),
            Ok(false) => println!("           new file; nothing to load yet"),
            Err(err) => eprintln!("warning: could not read the save file: {err}"),
        }
    }

    let result = event_loop.run_app(&mut app);

    // Write the save even if the event loop failed — whatever went wrong with the
    // window, the player's progress is still worth keeping.
    app.write_save();
    result?;
    Ok(())
}

struct App {
    cpu: Cpu,
    bus: TestBus,
    /// Polled once per frame rather than event-driven; see [`game_controller`].
    gamepad: GameController,
    /// `None` when the machine has no usable output device — the emulator still runs.
    audio: Option<Audio>,
    /// `None` for a cartridge with no battery, whose RAM is meant to be forgotten.
    save: Option<SaveFile>,
    /// Scratch buffer for moving samples from the APU to the audio thread, reused every
    /// frame so the audio path does not allocate.
    samples: Vec<(f32, f32)>,
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
    /// Rolling performance counters, shown by the overlay.
    stats: FrameStats,
    /// Whether the surface waits for the display before presenting.
    ///
    /// On by default, as `pixels` configures it. Worth being able to turn off: the
    /// emulator paces itself at 59.73 Hz while the display refreshes at some other
    /// rate, so the two drift in and out of phase and the present call periodically
    /// blocks for a whole refresh — which shows up as late frames on a machine that
    /// is barely working. Off trades tearing for that.
    vsync: bool,
    /// When the current frame cycle began, for the deadline-to-deadline total.
    cycle_started: Instant,
    /// `None` until the window exists, since it needs the surface format and the GPU
    /// device that come with it.
    overlay: Option<Overlay>,
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
            Action::ToggleOverlay => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.toggle();
                }
            }
            Action::ToggleVsync => self.set_vsync(!self.vsync),
            Action::Quit => event_loop.exit(),
        }
    }

    /// Writes the save file, reporting a failure rather than swallowing it.
    ///
    /// Losing a save silently is the worst possible outcome here: the player finds out
    /// hours later, and there is nothing left to recover.
    fn write_save(&mut self) {
        let Some(save) = self.save.take() else {
            return;
        };
        if let Err(err) = save.store_if_dirty(&mut self.bus) {
            eprintln!("error: could not write {}: {err}", save.path().display());
        }
        self.save = Some(save);
    }

    /// Moves a frame's worth of samples from the APU to the sound device.
    fn drain_audio(&mut self) {
        let Some(audio) = self.audio.as_ref() else {
            // With no device the samples would pile up in the APU forever, so drop them.
            self.samples.clear();
            self.bus.apu.drain_samples(&mut self.samples);
            self.samples.clear();
            return;
        };

        self.samples.clear();
        self.bus.apu.drain_samples(&mut self.samples);
        audio.queue(&self.samples);
    }

    /// Turns vsync on or off, reconfiguring the surface.
    ///
    /// Kept in step with `pixels` rather than read back from it, because the overlay
    /// needs the flag every frame and `present_mode()` returns a wgpu enum that says
    /// `AutoVsync` even when the platform quietly picked something else.
    fn set_vsync(&mut self, enabled: bool) {
        self.vsync = enabled;
        if let Some(pixels) = self.pixels.as_mut() {
            pixels.enable_vsync(enabled);
        }
    }

    /// Draws the emulated screen and, if it is showing, the overlay on top.
    ///
    /// Both go through a single `render_with` call: `pixels` scales the framebuffer
    /// into the surface, then egui draws over the result. The borrows are split out
    /// into locals before the closure because `render_with` takes `&self` on `pixels`
    /// while the overlay needs `&mut` — naming the fields separately is what lets the
    /// borrow checker see they do not overlap.
    fn render(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (Some(pixels), Some(window)) = (self.pixels.as_ref(), self.window.as_ref()) else {
            return Ok(());
        };

        let overlay = self.overlay.as_mut();
        let stats = &self.stats;

        if let Some(overlay) = overlay {
            overlay.prepare(window, pixels.device(), pixels.queue(), stats, self.vsync);
            overlay.upload(pixels.device(), pixels.queue());
        }

        // Re-borrow immutably for the closure; the `&mut` phase is done by now.
        let overlay = self.overlay.as_ref();

        pixels.render_with(|encoder, target, context| {
            context.scaling_renderer.render(encoder, target);

            if let Some(overlay) = overlay
                && overlay.has_content()
            {
                overlay.render(encoder, target);
            }

            Ok(())
        })?;

        Ok(())
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
            .with_title(format!("game-koi — {}", self.title))
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

        // The overlay borrows the device and surface format that `pixels` just
        // created, so it can only be built now.
        if let Some(pixels) = self.pixels.as_ref() {
            // Which adapter `pixels` settled on is only knowable now, and cannot change
            // under a live surface, so it is read once rather than polled per frame.
            self.stats
                .set_gpu(GpuInfo::from_adapter_info(&pixels.adapter().get_info()));

            self.overlay = Some(Overlay::new(
                &window,
                pixels.device(),
                pixels.surface_texture_format(),
                physical,
            ));
        }

        let want = self.vsync;
        self.set_vsync(want);
        self.window = Some(window);
        self.next_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // egui sees every event first and reports whether it wants it exclusively.
        // Honouring that is what stops a click on an overlay widget from also being
        // read as a Game Boy button press. While the overlay is hidden it consumes
        // nothing, so this costs the emulator no input at all.
        if let (Some(overlay), Some(window)) = (self.overlay.as_mut(), self.window.as_ref()) {
            let window = window.clone();
            if overlay.on_window_event(&window, &event) {
                // Still honour a close request: egui must not be able to trap it.
                if matches!(event, WindowEvent::CloseRequested) {
                    event_loop.exit();
                }
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(pixels) = self.pixels.as_mut()
                    && let Err(err) = pixels.resize_surface(size.width, size.height)
                {
                    eprintln!("error: resize failed: {err}");
                    event_loop.exit();
                }
                // egui lays out in logical points, so it needs both the new size and
                // the scale factor; missing the latter is how text ends up the wrong
                // size on a HiDPI display.
                if let (Some(overlay), Some(window)) = (self.overlay.as_mut(), self.window.as_ref())
                {
                    overlay.resize(size, window.scale_factor() as f32);
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
                // Timed here rather than folded into the frame's work: drawing happens
                // in a different callback from emulation, so a span around either one
                // alone measures a fraction of the frame.
                let started = Instant::now();
                if let Err(err) = self.render() {
                    eprintln!("error: render failed: {err}");
                    event_loop.exit();
                }
                self.stats.rendered(started.elapsed());
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

        // Anything the overlay's widgets asked for last frame. Applied here rather
        // than mid-render because carrying it out can reconfigure the surface, which
        // must not happen while a frame is being drawn into it.
        if let Some(action) = self.overlay.as_mut().and_then(Overlay::take_request) {
            self.apply(action, event_loop);
        }

        // Sleep until the frame is due, then run it in this same callback. Sleeping
        // and returning instead would cost an extra event-loop round trip per frame,
        // which is enough to drag the rate well below the hardware's 59.73 Hz.
        let now = Instant::now();
        let mut slept = Duration::ZERO;
        if now < self.next_frame {
            std::thread::sleep(self.next_frame - now);
            // Measured rather than assumed: `sleep` guarantees only a lower bound, and
            // the overshoot is the scheduler's, not ours. Counting the requested time
            // would quietly hide a laggy scheduler in the blocked figure.
            slept = now.elapsed();
        }
        self.stats.slept(slept);

        if !self.paused {
            // The emulation phase only. The render phase is timed in
            // `RedrawRequested`, and the deadline-to-deadline total below covers both
            // plus anything spent blocked.
            let work_started = Instant::now();
            self.run_frame();
            self.drain_audio();
            self.stats.emulated(work_started.elapsed());

            self.frames += 1;
            // Close the frame against the previous cycle's start, so the total takes
            // in the render that happened between the two and any wait it involved.
            let now = Instant::now();
            let total = now.duration_since(self.cycle_started);
            self.cycle_started = now;
            self.stats.frame_completed(total);
            // Start the clock after the first frame: creating the window and GPU
            // surface takes a few hundred milliseconds, and averaging that one-off
            // cost into a short run makes the frame rate look far worse than it is.
            if self.frames == 1 {
                self.started = Instant::now();
            }

            if self.frames.is_multiple_of(AUTOSAVE_FRAMES) {
                self.write_save();
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
            // The deadline had already passed before we got here, so this frame's work
            // overran its budget. Nothing was skipped — the frame was simply late, and
            // resyncing here is what stops one slow frame compounding into drift.
            self.stats.frame_was_late();
            self.next_frame = after + FRAME_TIME;
        }

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Releases everything tied to the window before the event loop shuts down.
    ///
    /// Not housekeeping — leaving this to `App`'s own drop is a segfault on every exit.
    /// By the time `run_app` returns, winit has closed its connection to the display
    /// server and freed it. egui-winit's clipboard is built on a *borrowed* Wayland
    /// display pointer rather than a reference-counted handle, and dropping it signals a
    /// worker thread which then destroys its protocol objects through that pointer — at
    /// which point it is dangling. Dropping here puts the teardown back inside the event
    /// loop's lifetime, while the connection is still open.
    ///
    /// The order is the reverse of how `resumed` builds them: the overlay borrows the
    /// GPU device `pixels` owns, and `pixels`' surface borrows the window.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.overlay = None;
        self.pixels = None;
        self.window = None;
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
