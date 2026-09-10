//! Host-side performance counters.
//!
//! Everything here describes the *emulator*, not the emulated machine: how fast we are
//! managing to run, and whether we are keeping up. A Game Boy has no idea what a
//! wall clock is, so none of this belongs in the core — the DMG's own notion of a
//! frame is 70224 T-cycles, and that number never varies no matter how slowly we
//! execute them.
//!
//! # What "late" means here
//!
//! No frame is ever skipped: every frame the PPU completes gets drawn. What varies is
//! whether it was drawn *on time*. A frame is late when the whole cycle overran the
//! 16.74 ms budget, leaving nothing to sleep away — so the next frame starts behind.
//! It is deliberately not called "dropped": nothing was thrown away.
//!
//! Late does **not** on its own mean the machine is too slow. With vsync on, the
//! present call blocks until the compositor frees a swapchain image, and a 59.73 Hz
//! emulator against a 60 Hz display drifts in and out of phase forever — so frames go
//! late on a machine using a fraction of one core. That is why the phases are timed
//! separately: `emulate` and `render` are work this program does, the pacing sleep is
//! time given back deliberately, and whatever is left is the frame blocked on someone
//! else.
//!
//! # Why the phases are timed separately
//!
//! The work is split across two winit callbacks — emulation in `about_to_wait`,
//! drawing in `RedrawRequested` — so a single span around either one measures a
//! fraction of the frame and reads as comfortable no matter how bad things get. The
//! total is measured deadline-to-deadline instead, and the phases are accumulated
//! into it.
//!
//! # Why the adapter belongs here too
//!
//! [`GpuInfo`] is the one thing in this module that is not a measurement: it is read
//! once and never changes. It sits next to the timings because it is the first thing
//! to check when they look wrong, and nothing in the frame breakdown says it. Only the
//! plain description is kept — no wgpu handles — so the module still needs no GPU to
//! test.

use std::time::{Duration, Instant};

// The same wgpu `pixels` is built on, re-exported by it. Only the adapter's
// self-description is touched here, which is plain data.
use pixels::wgpu::{AdapterInfo, Backend, DeviceType};

/// How long to gather frames before recomputing the rate.
///
/// A rate computed over a single frame is pure noise — one scheduler hiccup and it
/// reads 12 fps. Averaging over about a second gives a number steady enough to read.
const SAMPLE_WINDOW: Duration = Duration::from_millis(500);

/// Where a frame's time went.
///
/// `total` is measured deadline-to-deadline and so includes both phases plus anything
/// spent blocked; the phases are timed individually. `total - emulate - render` is
/// therefore the wait, which under vsync is the compositor holding the swapchain.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameTiming {
    /// Running the CPU until the PPU finishes a frame, plus draining audio.
    pub emulate: Duration,
    /// Scaling the framebuffer and drawing the overlay.
    pub render: Duration,
    /// The deliberate pacing sleep: the budget left over after the work, given back
    /// to the OS on purpose. A *large* value here is health, not a problem — it is
    /// the headroom.
    pub sleep: Duration,
    /// The whole cycle, including the sleep and anything spent blocked.
    pub total: Duration,
}

impl FrameTiming {
    /// Time that is neither work nor the deliberate sleep.
    ///
    /// This is the figure worth watching: it is the frame blocked on something
    /// outside this program, which under vsync means the compositor holding on to the
    /// swapchain. The pacing sleep is excluded precisely because it is intentional —
    /// lumping the two together makes a perfectly healthy frame look like it spends
    /// 14 ms stalled.
    ///
    /// Saturating because the phases and the total are sampled from different clocks
    /// in different callbacks; rounding can leave the sum a hair above the total, and
    /// a negative duration would panic.
    pub fn blocked(&self) -> Duration {
        self.total
            .saturating_sub(self.emulate)
            .saturating_sub(self.render)
            .saturating_sub(self.sleep)
    }
}

/// What sort of device the adapter turned out to be.
///
/// The distinction that earns this its own type is the last one: `Software` means there
/// is no GPU in the picture at all and a CPU rasteriser is drawing every pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Discrete,
    Integrated,
    /// Paravirtualised — a guest's view of a host GPU, so hardware underneath.
    Virtual,
    /// A CPU rasteriser: llvmpipe or lavapipe on Linux, WARP on Windows.
    Software,
    /// The adapter declined to say, which is wgpu's `DeviceType::Other`.
    Unknown,
}

impl GpuKind {
    /// Whether a GPU is doing the work.
    ///
    /// `None` when the adapter would not say, which is worth keeping distinct from a
    /// confident "no": reporting software rendering because a driver was reticent would
    /// send someone chasing a problem they do not have.
    pub fn accelerated(self) -> Option<bool> {
        match self {
            GpuKind::Discrete | GpuKind::Integrated | GpuKind::Virtual => Some(true),
            GpuKind::Software => Some(false),
            GpuKind::Unknown => None,
        }
    }

    /// How to name it in the panel.
    pub fn label(self) -> &'static str {
        match self {
            GpuKind::Discrete => "discrete GPU",
            GpuKind::Integrated => "integrated GPU",
            GpuKind::Virtual => "virtual GPU",
            GpuKind::Software => "software (CPU)",
            GpuKind::Unknown => "unknown",
        }
    }
}

/// Which device is drawing the frames, and through which driver.
///
/// Read once from the wgpu adapter when the surface is created — an adapter cannot
/// change under a live surface, so there is nothing to poll per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    /// The adapter's own name for itself, e.g. "NVIDIA GeForce RTX 3070 Ti Laptop GPU".
    pub device: String,
    /// Whether real graphics hardware is involved, and of what sort.
    pub kind: GpuKind,
    /// The graphics API wgpu chose to talk to the device through.
    pub backend: &'static str,
    /// Driver name and version, as the adapter reports them.
    pub driver: String,
}

impl GpuInfo {
    /// Reads the facts out of a wgpu adapter's self-description.
    ///
    /// Both enums are matched exhaustively on purpose: neither is `#[non_exhaustive]`,
    /// so a new wgpu variant breaks the build here rather than being quietly folded into
    /// "unknown" and misreporting what the machine is doing.
    pub fn from_adapter_info(info: &AdapterInfo) -> Self {
        GpuInfo {
            device: info.name.clone(),
            kind: match info.device_type {
                DeviceType::DiscreteGpu => GpuKind::Discrete,
                DeviceType::IntegratedGpu => GpuKind::Integrated,
                DeviceType::VirtualGpu => GpuKind::Virtual,
                DeviceType::Cpu => GpuKind::Software,
                DeviceType::Other => GpuKind::Unknown,
            },
            // wgpu's own names are lowercase identifiers (`dx12`, `gl`); these are for
            // a person to read.
            backend: match info.backend {
                Backend::Vulkan => "Vulkan",
                Backend::Metal => "Metal",
                Backend::Dx12 => "Direct3D 12",
                Backend::Gl => "OpenGL",
                Backend::BrowserWebGpu => "WebGPU",
                Backend::Noop => "none",
            },
            driver: join_driver(&info.driver, &info.driver_info),
        }
    }
}

/// Joins a driver's name and version, tolerating either being missing.
///
/// Which of the two an adapter fills in is entirely up to it: Mesa sets both ("radv",
/// "Mesa 25.2.1"), NVIDIA's proprietary driver reports its version in `driver_info`, and
/// some report neither. Formatting them with a fixed separator leaves a stray space or a
/// bare version number on display, so the empty cases are handled explicitly.
fn join_driver(name: &str, version: &str) -> String {
    match (name.trim(), version.trim()) {
        ("", "") => "unknown".to_string(),
        ("", version) => version.to_string(),
        (name, "") => name.to_string(),
        (name, version) => format!("{name} {version}"),
    }
}

/// Rolling performance counters, recomputed roughly twice a second.
pub struct FrameStats {
    /// Frames completed since the current sampling window opened.
    frames_in_window: u32,
    /// When that window opened.
    window_started: Instant,
    /// The most recent measured frame rate.
    fps: f64,
    /// Where the last frame's time went.
    ///
    /// With pacing working, [`fps`](Self::fps) sits at 59.7 whatever the machine is
    /// doing, because the sleep absorbs the slack. This is what says how much slack
    /// there was, and which phase used it.
    timing: FrameTiming,
    /// Emulation time accumulated for the frame currently in progress.
    ///
    /// Held here because emulation and rendering happen in different callbacks: the
    /// emulate phase is recorded first and has to survive until the frame closes.
    pending_emulate: Duration,
    /// Render time for the frame in progress, likewise.
    pending_render: Duration,
    /// The pacing sleep for the frame in progress.
    pending_sleep: Duration,
    /// Frames whose total overran the frame budget.
    late_frames: u64,
    /// Frames run in total, so the late count can be read as a proportion.
    total_frames: u64,
    /// What is drawing the frames. `None` until the surface exists, since there is no
    /// adapter to ask before then — the counters start before the window does.
    gpu: Option<GpuInfo>,
}

impl FrameStats {
    pub fn new() -> Self {
        FrameStats {
            frames_in_window: 0,
            window_started: Instant::now(),
            fps: 0.0,
            timing: FrameTiming::default(),
            pending_emulate: Duration::ZERO,
            pending_render: Duration::ZERO,
            pending_sleep: Duration::ZERO,
            late_frames: 0,
            total_frames: 0,
            gpu: None,
        }
    }

    /// Records what the surface ended up rendering on, once that is known.
    pub fn set_gpu(&mut self, gpu: GpuInfo) {
        self.gpu = Some(gpu);
    }

    /// What is drawing the frames, or `None` before the surface exists.
    pub fn gpu(&self) -> Option<&GpuInfo> {
        self.gpu.as_ref()
    }

    /// Records how long emulating this frame took.
    pub fn emulated(&mut self, elapsed: Duration) {
        self.pending_emulate = elapsed;
    }

    /// Records how long drawing took.
    ///
    /// Accumulates rather than overwrites: winit can ask for more than one redraw
    /// between frames, and every one of them costs GPU time that belongs to this
    /// frame.
    pub fn rendered(&mut self, elapsed: Duration) {
        self.pending_render += elapsed;
    }

    /// Records how long the frame slept waiting for its deadline.
    pub fn slept(&mut self, elapsed: Duration) {
        self.pending_sleep = elapsed;
    }

    /// Closes out a frame, given the deadline-to-deadline total.
    pub fn frame_completed(&mut self, total: Duration) {
        self.timing = FrameTiming {
            emulate: self.pending_emulate,
            render: self.pending_render,
            sleep: self.pending_sleep,
            total,
        };
        self.pending_emulate = Duration::ZERO;
        self.pending_render = Duration::ZERO;
        self.pending_sleep = Duration::ZERO;

        self.total_frames += 1;
        self.frames_in_window += 1;

        let elapsed = self.window_started.elapsed();
        if elapsed >= SAMPLE_WINDOW {
            self.fps = f64::from(self.frames_in_window) / elapsed.as_secs_f64();
            self.frames_in_window = 0;
            self.window_started = Instant::now();
        }
    }

    /// Records that a frame missed its deadline.
    pub fn frame_was_late(&mut self) {
        self.late_frames += 1;
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn timing(&self) -> FrameTiming {
        self.timing
    }

    pub fn late_frames(&self) -> u64 {
        self.late_frames
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Speed as a percentage of real hardware.
    ///
    /// More legible than a raw rate: 100% is a DMG, and anything else is immediately
    /// readable as "too slow" or "running fast" without knowing that the hardware's
    /// odd refresh rate is 59.73 Hz rather than 60.
    pub fn speed_percent(&self, target_hz: f64) -> f64 {
        self.fps / target_hz * 100.0
    }

    /// How much of the frame budget the work consumed, as a fraction.
    ///
    /// Above 1.0 means the frame could not have been paced — the work alone exceeded
    /// the budget. This is the headroom figure worth watching.
    pub fn budget_used(&self, budget: Duration) -> f64 {
        self.timing.total.as_secs_f64() / budget.as_secs_f64()
    }

    /// The share of the budget this program actually spent working.
    ///
    /// The figure that answers "is my machine fast enough": it excludes time spent
    /// blocked on the compositor, which no amount of CPU would help with.
    pub fn work_used(&self, budget: Duration) -> f64 {
        (self.timing.emulate + self.timing.render).as_secs_f64() / budget.as_secs_f64()
    }
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a completed frame from its three parts.
    fn frame(
        stats: &mut FrameStats,
        emulate_ms: u64,
        render_ms: u64,
        sleep_ms: u64,
        total_ms: u64,
    ) {
        stats.emulated(Duration::from_millis(emulate_ms));
        stats.rendered(Duration::from_millis(render_ms));
        stats.slept(Duration::from_millis(sleep_ms));
        stats.frame_completed(Duration::from_millis(total_ms));
    }

    #[test]
    fn late_frames_are_counted_separately_from_total() {
        let mut stats = FrameStats::new();
        frame(&mut stats, 2, 1, 1, 4);
        frame(&mut stats, 2, 1, 0, 30);
        stats.frame_was_late();

        assert_eq!(stats.total_frames(), 2);
        assert_eq!(
            stats.late_frames(),
            1,
            "only the overrun should count as late"
        );
    }

    #[test]
    fn a_healthy_frame_is_not_blocked_at_all() {
        // 2ms of work, the rest slept on purpose. The sleep is the headroom, so
        // counting it as a stall would make every healthy frame look pathological.
        let mut stats = FrameStats::new();
        frame(&mut stats, 2, 1, 13, 16);

        assert_eq!(stats.timing().blocked(), Duration::ZERO);
    }

    #[test]
    fn blocking_shows_up_when_the_frame_overruns_its_sleep() {
        // The vsync case: the same small workload and a short sleep, but the frame
        // still took a full refresh because the present call blocked.
        let mut stats = FrameStats::new();
        frame(&mut stats, 2, 1, 3, 16);

        assert_eq!(stats.timing().blocked(), Duration::from_millis(10));
    }

    #[test]
    fn blocked_never_underflows_when_the_parts_exceed_the_total() {
        // The phases and the total come from different clocks in different callbacks,
        // so their sum can land just above the total. A naive subtraction would panic.
        let mut stats = FrameStats::new();
        frame(&mut stats, 10, 10, 5, 15);

        assert_eq!(stats.timing().blocked(), Duration::ZERO);
    }

    #[test]
    fn work_and_budget_diverge_when_blocked() {
        // The distinction the panel exists to draw: a frame can consume its whole
        // budget while the machine is nearly idle.
        let budget = Duration::from_millis(16);
        let mut stats = FrameStats::new();
        frame(&mut stats, 2, 1, 3, 16);

        assert!(
            stats.budget_used(budget) >= 1.0,
            "the frame used its budget"
        );
        assert!(
            stats.work_used(budget) < 0.25,
            "but the work was a fraction of it, so the CPU is not the problem"
        );
    }

    #[test]
    fn render_time_accumulates_across_multiple_redraws() {
        // winit can ask for more than one redraw per frame, and each costs GPU time
        // that belongs to the same frame.
        let mut stats = FrameStats::new();
        stats.emulated(Duration::from_millis(2));
        stats.rendered(Duration::from_millis(1));
        stats.rendered(Duration::from_millis(3));
        stats.slept(Duration::from_millis(10));
        stats.frame_completed(Duration::from_millis(16));

        assert_eq!(stats.timing().render, Duration::from_millis(4));
    }

    #[test]
    fn phases_reset_between_frames() {
        let mut stats = FrameStats::new();
        frame(&mut stats, 5, 5, 6, 16);
        frame(&mut stats, 1, 1, 14, 16);

        assert_eq!(stats.timing().emulate, Duration::from_millis(1));
        assert_eq!(stats.timing().render, Duration::from_millis(1));
    }

    #[test]
    fn budget_used_crosses_one_when_a_frame_exceeds_the_budget() {
        let budget = Duration::from_millis(16);
        let mut stats = FrameStats::new();

        frame(&mut stats, 4, 2, 2, 8);
        assert!(
            stats.budget_used(budget) < 1.0,
            "half the budget is headroom"
        );

        frame(&mut stats, 15, 5, 0, 20);
        assert!(
            stats.budget_used(budget) > 1.0,
            "work beyond the budget cannot be paced"
        );
    }

    #[test]
    fn speed_is_a_percentage_of_the_hardware_rate() {
        let mut stats = FrameStats::new();
        // Force a rate without waiting for the sampling window: half of 59.73 Hz
        // should read as 50%, not 50 fps.
        stats.fps = 29.8635;
        assert!((stats.speed_percent(59.727) - 50.0).abs() < 0.1);
    }

    /// An adapter's self-description, with the fields this module reads filled in.
    ///
    /// `AdapterInfo` is plain data with public fields, so this needs no GPU at all.
    fn adapter_info(name: &str, device_type: DeviceType) -> AdapterInfo {
        AdapterInfo {
            name: name.to_string(),
            vendor: 0,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: "radv".to_string(),
            driver_info: "Mesa 25.2.1".to_string(),
            backend: Backend::Vulkan,
            // Nothing here reads these; they are listed because `AdapterInfo` has no
            // `Default`, so a wgpu upgrade that adds a field breaks this helper and
            // nothing else.
            subgroup_min_size: 32,
            subgroup_max_size: 32,
            transient_saves_memory: false,
        }
    }

    #[test]
    fn a_cpu_adapter_is_reported_as_not_accelerated() {
        // The case the whole type exists for: llvmpipe and lavapipe report themselves as
        // `Cpu`, and that is the answer to "why is the render phase so large".
        let info = GpuInfo::from_adapter_info(&adapter_info("llvmpipe", DeviceType::Cpu));
        assert_eq!(info.kind, GpuKind::Software);
        assert_eq!(info.kind.accelerated(), Some(false));
    }

    #[test]
    fn real_hardware_is_reported_as_accelerated() {
        for device_type in [
            DeviceType::DiscreteGpu,
            DeviceType::IntegratedGpu,
            DeviceType::VirtualGpu,
        ] {
            let info = GpuInfo::from_adapter_info(&adapter_info("gpu", device_type));
            assert_eq!(
                info.kind.accelerated(),
                Some(true),
                "{device_type:?} has hardware behind it"
            );
        }
    }

    #[test]
    fn an_unspecific_adapter_is_neither_yes_nor_no() {
        // `Other` means the driver would not say. Guessing "software" here would send
        // someone chasing a problem they do not have.
        let info = GpuInfo::from_adapter_info(&adapter_info("?", DeviceType::Other));
        assert_eq!(info.kind, GpuKind::Unknown);
        assert_eq!(info.kind.accelerated(), None);
    }

    #[test]
    fn the_backend_is_named_for_a_person_to_read() {
        let info = GpuInfo::from_adapter_info(&adapter_info("gpu", DeviceType::DiscreteGpu));
        // wgpu's own name for this is the lowercase identifier "vulkan".
        assert_eq!(info.backend, "Vulkan");
    }

    #[test]
    fn driver_name_and_version_are_joined_without_stray_separators() {
        // Which of the two an adapter fills in varies by driver, so all four
        // combinations have to read properly.
        assert_eq!(join_driver("radv", "Mesa 25.2.1"), "radv Mesa 25.2.1");
        assert_eq!(join_driver("NVIDIA", ""), "NVIDIA");
        assert_eq!(join_driver("", "610.57.04"), "610.57.04");
        assert_eq!(join_driver("", ""), "unknown");
    }

    #[test]
    fn the_gpu_is_unknown_until_the_surface_exists() {
        // The counters are constructed before the window, so the panel has to cope with
        // there being no adapter to describe yet.
        assert!(FrameStats::new().gpu().is_none());
    }

    #[test]
    fn the_rate_stays_zero_until_a_window_closes() {
        // Reporting a rate computed from one frame would be noise, so nothing is
        // reported until enough frames have accumulated to mean something.
        let mut stats = FrameStats::new();
        frame(&mut stats, 2, 1, 13, 16);
        assert_eq!(stats.fps(), 0.0);
    }
}
