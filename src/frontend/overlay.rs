//! The egui debug overlay.
//!
//! Draws on top of the emulated screen inside the same render pass, so there is no
//! second window and nothing for the core to know about. Like the rest of the
//! frontend, this is outside the emulated machine entirely.
//!
//! # Why egui 0.35 and not the current release
//!
//! `pixels` 0.17 builds on wgpu 29, and this module borrows that same `wgpu::Device`
//! and `Queue` rather than creating its own. egui-wgpu 0.36 moved to wgpu 30, which
//! is semver-incompatible: Cargo would link *two* wgpu crates, and the `wgpu::Device`
//! from `pixels` would be a different type from the one egui wants, producing the
//! memorably unhelpful error "expected `wgpu::Device`, found `wgpu::Device`". Pinning
//! to 0.35 keeps a single wgpu in the tree. `cargo tree -d` is the check.
//!
//! # Drawing order
//!
//! The overlay renders in the same pass as the game, after it: `pixels` scales the
//! 160x144 framebuffer into the surface, then egui draws over the result. Doing it in
//! one pass rather than two avoids a second surface acquisition per frame.

use egui_wgpu::wgpu;
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use egui_winit::State;
use winit::window::Window;

use super::Action;
use super::stats::FrameStats;

/// egui's context, its winit input translation, and its wgpu renderer.
pub struct Overlay {
    context: egui::Context,
    /// Translates winit events into egui input, and reports whether egui wants them.
    state: State,
    renderer: Renderer,
    /// The tessellated output of the last `run`, waiting to be drawn.
    paint_jobs: Vec<egui::ClippedPrimitive>,
    descriptor: ScreenDescriptor,
    /// Whether the panel is showing. Hidden by default: the overlay is a debugging
    /// tool, and covering a corner of a 160x144 screen is a real cost.
    visible: bool,
    /// What a widget asked for this frame, for the caller to carry out.
    request: Option<Action>,
}

impl Overlay {
    pub fn new(
        window: &Window,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: winit::dpi::PhysicalSize<u32>,
    ) -> Self {
        let context = egui::Context::default();
        let state = State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            // Let egui size its texture atlas to whatever this GPU allows.
            Some(device.limits().max_texture_dimension_2d as usize),
        );

        let renderer = Renderer::new(
            device,
            format,
            RendererOptions {
                // egui feathers its own edges, and there is no 3D here to alias.
                msaa_samples: 1,
                ..Default::default()
            },
        );

        Overlay {
            context,
            state,
            renderer,
            paint_jobs: Vec::new(),
            descriptor: ScreenDescriptor {
                size_in_pixels: [size.width, size.height],
                pixels_per_point: window.scale_factor() as f32,
            },
            visible: false,
            request: None,
        }
    }

    /// Takes whatever a widget asked for this frame, if anything.
    pub fn take_request(&mut self) -> Option<Action> {
        self.request.take()
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Feeds a window event to egui, reporting whether egui wants it exclusively.
    ///
    /// The caller must not also act on a consumed event: once there are widgets to
    /// click, a click on a slider would otherwise press a Game Boy button as well.
    /// While the overlay is hidden nothing can be consumed, so the emulator sees
    /// everything.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        if !self.visible {
            return false;
        }
        self.state.on_window_event(window, event).consumed
    }

    /// Keeps the descriptor in step with the window.
    ///
    /// egui lays out in logical points while the surface is in physical pixels, so
    /// both the size and the scale factor have to be tracked — miss the latter and
    /// the text comes out the wrong size on a HiDPI display.
    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>, scale_factor: f32) {
        self.descriptor.size_in_pixels = [size.width, size.height];
        self.descriptor.pixels_per_point = scale_factor;
    }

    /// Runs the UI and tessellates it, ready for [`Overlay::render`].
    ///
    /// `device` and `queue` come from `pixels`: this module deliberately does not own
    /// a GPU of its own, which is the whole reason the wgpu versions have to line up.
    pub fn prepare(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stats: &FrameStats,
        vsync: bool,
    ) {
        if !self.visible {
            self.paint_jobs.clear();
            return;
        }

        // Widgets cannot reach into `App`, so a click sets this and the caller picks
        // it up through `take_request` — the same one-way flow the input devices use.
        let mut request = None;
        let input = self.state.take_egui_input(window);
        let output = self
            .context
            .run_ui(input, |ui| stats_panel(ui, stats, vsync, &mut request));
        self.request = request;

        self.state
            .handle_platform_output(window, output.platform_output);

        self.paint_jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);

        // Upload any glyphs egui rasterised this frame before drawing with them.
        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }

    /// Whether there is anything to draw, so the caller can skip the pass entirely.
    pub fn has_content(&self) -> bool {
        !self.paint_jobs.is_empty()
    }

    /// Uploads egui's vertex and index data to the GPU.
    ///
    /// Separate from [`Overlay::render`] and called during the `&mut` phase, because
    /// this is the only part of drawing that needs `&mut self` — `render` itself takes
    /// `&self`, which is what lets it run inside `pixels`' `render_with` closure.
    ///
    /// It needs an encoder of its own: egui may stage buffer copies here, and the
    /// encoder `render_with` provides is not available until we are already inside the
    /// closure with only `&self` in hand. Submitting this one first keeps the copies
    /// ordered before the draw that reads them.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.paint_jobs.is_empty() {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("egui_upload"),
        });
        let extra = self.renderer.update_buffers(
            device,
            queue,
            &mut encoder,
            &self.paint_jobs,
            &self.descriptor,
        );
        // `update_buffers` can hand back its own command buffers; ours goes last so
        // any copies it recorded run before the ones we just encoded.
        queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
    }

    /// Draws the overlay over whatever is already in `target`.
    ///
    /// `LoadOp::Load` rather than `Clear`: the emulated screen has just been scaled
    /// into this texture and we are drawing on top of it, not replacing it.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui_overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // egui-wgpu wants a 'static pass. The pass does borrow the encoder, but it is
        // dropped at the end of this function, well before the encoder is submitted —
        // `forget_lifetime` is how wgpu 29 lets us say so.
        let mut pass = pass.forget_lifetime();
        self.renderer
            .render(&mut pass, &self.paint_jobs, &self.descriptor);
    }
}

/// The stats panel itself.
///
/// Free function rather than a method so it borrows nothing but what it draws.
/// `request` is how a click gets back out to the caller.
fn stats_panel(ui: &mut egui::Ui, stats: &FrameStats, vsync: bool, request: &mut Option<Action>) {
    let timing = stats.timing();

    egui::Window::new("Stats")
        .default_pos([8.0, 8.0])
        .resizable(false)
        .show(ui.ctx(), |ui| {
            egui::Grid::new("stats_grid")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    ui.label("FPS");
                    ui.label(format!("{:.1}", stats.fps()));
                    ui.end_row();

                    ui.label("Speed");
                    ui.label(format!("{:.0}%", stats.speed_percent(super::TARGET_HZ)));
                    ui.end_row();

                    ui.label("Frame");
                    ui.label(millis(timing.total));
                    ui.end_row();
                });

            ui.separator();

            // The breakdown. `emulate` and `render` are work this program does;
            // `waiting` is not, so a large figure there is not a CPU problem and no
            // faster machine would shrink it.
            egui::Grid::new("phase_grid")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    ui.label("  emulate");
                    ui.label(millis(timing.emulate));
                    ui.end_row();

                    ui.label("  render");
                    ui.label(millis(timing.render));
                    ui.end_row();

                    ui.label("  sleep");
                    ui.label(millis(timing.sleep));
                    ui.end_row();

                    // The one to watch. Work is this program's cost and sleep is
                    // headroom given back on purpose; anything left over is the frame
                    // held up by something outside it.
                    ui.label("  blocked");
                    let blocked = timing.blocked();
                    let colour = if blocked > std::time::Duration::from_millis(2) {
                        egui::Color32::YELLOW
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.colored_label(colour, millis(blocked));
                    ui.end_row();
                });

            ui.separator();

            egui::Grid::new("budget_grid")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    // Two budget figures, because they answer different questions.
                    // "Work" is whether the machine is fast enough; "frame" is whether
                    // the frame landed on time. They diverge exactly when something
                    // outside this program is holding it up.
                    ui.label("Work");
                    let work = stats.work_used(super::FRAME_TIME);
                    ui.colored_label(load_colour(ui, work), percent(work));
                    ui.end_row();

                    ui.label("Frame budget");
                    let used = stats.budget_used(super::FRAME_TIME);
                    ui.colored_label(load_colour(ui, used), percent(used));
                    ui.end_row();

                    ui.label("Late");
                    ui.label(format!(
                        "{} of {}",
                        stats.late_frames(),
                        stats.total_frames()
                    ));
                    ui.end_row();
                });

            ui.separator();

            let mut on = vsync;
            if ui
                .checkbox(&mut on, "Vsync")
                .on_hover_text(
                    "Waits for the display before presenting. Because the emulator \
                     paces itself at 59.73 Hz and the display refreshes at its own \
                     rate, the two drift in and out of phase and this periodically \
                     blocks for a full refresh — which shows up as waiting time and \
                     late frames even on a fast machine. Turning it off removes that \
                     at the cost of tearing.",
                )
                .changed()
            {
                *request = Some(Action::ToggleVsync);
            }
        });
}

/// A duration in milliseconds, at a fixed width so the column does not jitter.
fn millis(d: std::time::Duration) -> String {
    format!("{:>6.2} ms", d.as_secs_f64() * 1000.0)
}

fn percent(fraction: f64) -> String {
    format!("{:>4.0}%", fraction * 100.0)
}

/// Green through to red as a figure approaches and passes its budget.
///
/// Only worth colouring near the limit: a number that matters at 100% is easy to miss
/// when it spends most of its time at 15%.
fn load_colour(ui: &egui::Ui, fraction: f64) -> egui::Color32 {
    if fraction > 1.0 {
        egui::Color32::RED
    } else if fraction > 0.8 {
        egui::Color32::YELLOW
    } else {
        ui.visuals().text_color()
    }
}
