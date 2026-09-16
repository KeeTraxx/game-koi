/**
 * Host-side performance counters for the browser build.
 *
 * Everything here describes the *emulator*, not the emulated machine: how fast the page
 * is managing to run it, and whether it is keeping up. A Game Boy has no idea what a
 * wall clock is — its own notion of a frame is 70224 T-cycles, and that number never
 * varies however slowly they are executed.
 *
 * This is the browser counterpart of `game-koi-desktop`'s `stats.rs`, and it counts
 * different things on purpose, because the two frontends are paced differently.
 *
 * # Frames are not wakes
 *
 * The desktop sleeps until each 16.74 ms deadline, so one loop iteration is one frame
 * and a frame rate is just how often the loop ran. Here `requestAnimationFrame` wakes at
 * the *display's* rate — 120 Hz on a fast panel, 0 Hz in a backgrounded tab — and each
 * wake emulates however many frames the audio buffer is short by, which may be two, one
 * or none. Counting wakes would report the refresh rate and say nothing about the
 * emulator, so `fps` counts frames the emulator actually completed. `framesPerWake` is
 * the ratio between the two clocks: 1.0 on a 60 Hz display, ~0.5 on a 120 Hz one.
 *
 * # There is no "blocked" figure here
 *
 * The desktop's most useful number is time neither spent working nor slept away, which
 * is the compositor holding the swapchain. A page never sleeps and never presents, so
 * that figure has no analogue. Its replacement is the audio buffer: since pacing runs
 * off the sound card, "am I keeping up" *is* "is the buffer staying full", and a buffer
 * that reaches zero is the audible failure the whole design exists to avoid. That is why
 * `underruns` is the closest thing here to the desktop's late-frame count.
 *
 * # The clock is passed in, never read
 *
 * Nothing in this module calls `performance.now()`; the caller passes the timestamp it
 * already has. That keeps the counters pure — testable with plain numbers, no timers and
 * no fake clock.
 */

/** The DMG's frame rate. Not 60: 4194304 / 70224 cycles per frame. */
export const TARGET_HZ = 59.7275;

/** The wall-clock budget for one emulated frame, in milliseconds. */
export const FRAME_MS = 1000 / TARGET_HZ;

/**
 * How long to gather before recomputing the averages.
 *
 * A rate computed over a single wake is pure noise — one scheduler hiccup and it reads
 * 12 fps. Averaging over about half a second gives numbers steady enough to read, which
 * matters more here than on the desktop because these are rendered as text that a person
 * is trying to follow rather than a graph.
 */
const SAMPLE_WINDOW_MS = 500;

/** What one `requestAnimationFrame` wake did. */
export interface WakeReport {
  /** Milliseconds spent running the emulator and handing its samples to the worklet. */
  emulate: number;
  /** Milliseconds spent blitting to the canvas; zero when nothing was drawn. */
  draw: number;
  /** Frames emulated this wake — often 0 or 1, more while catching up. */
  frames: number;
  /** Whether `maxFramesPerWake` clamped the catch-up, leaving a deficit behind. */
  capped: boolean;
}

/** The audio side's view of itself, as last reported by the worklet. */
export interface AudioReport {
  /** Sample frames sitting in the worklet's ring, waiting to be played. */
  buffered: number;
  /** How many the page tries to keep there. */
  target: number;
  /** Cumulative silent samples emitted because the ring was empty. */
  underruns: number;
  /** Cumulative samples overwritten because the ring was full. */
  dropped: number;
  /** The `AudioContext`'s rate — what the core resamples its output to. */
  sampleRate: number;
  /** `"running"`, `"suspended"` or `"closed"`. */
  state: string;
}

/** Everything the panel draws, computed once per refresh. */
export interface StatsSnapshot {
  /** Emulated frames per second, averaged over the sampling window. */
  fps: number;
  /** `fps` as a percentage of real hardware, where 100% is a DMG. */
  speedPercent: number;
  /** Milliseconds of emulation per emulated frame. */
  emulate: number;
  /** Milliseconds of canvas blitting per emulated frame. */
  draw: number;
  /** `emulate + draw`, the work this page does per frame. */
  work: number;
  /** That work as a fraction of the 16.74 ms a frame is worth. */
  workFraction: number;
  /** Emulated frames per `rAF` wake: the ratio of the two clocks. */
  framesPerWake: number;
  /** How often `rAF` is firing — in practice, the display's refresh rate. */
  wakeRate: number;
  frames: number;
  wakes: number;
  /** Wakes whose catch-up hit `maxFramesPerWake`. */
  cappedWakes: number;
  /** `null` until the worklet has reported once. */
  audio: (AudioReport & { bufferedMs: number }) | null;
}

/** Rolling counters, recomputed roughly twice a second. */
export class FrameStats {
  private windowStart: number;
  private framesInWindow = 0;
  private wakesInWindow = 0;
  private emulateInWindow = 0;
  private drawInWindow = 0;

  private fps = 0;
  private wakeRate = 0;
  private emulatePerFrame = 0;
  private drawPerFrame = 0;
  private framesPerWake = 0;

  private frames = 0;
  private wakes = 0;
  private cappedWakes = 0;
  private audio: AudioReport | null = null;

  constructor(now: number) {
    this.windowStart = now;
  }

  /** Records a wake and, once the window is old enough, recomputes the averages. */
  wake(report: WakeReport, now: number): void {
    this.wakes++;
    this.wakesInWindow++;
    this.frames += report.frames;
    this.framesInWindow += report.frames;
    this.emulateInWindow += report.emulate;
    this.drawInWindow += report.draw;
    if (report.capped) this.cappedWakes++;

    const elapsed = now - this.windowStart;
    if (elapsed < SAMPLE_WINDOW_MS) return;

    const seconds = elapsed / 1000;
    this.fps = this.framesInWindow / seconds;
    this.wakeRate = this.wakesInWindow / seconds;
    // Per *frame*, not per wake: a wake that emulated nothing did no work, and
    // averaging those in would report a machine faster than it is.
    const frames = this.framesInWindow;
    this.emulatePerFrame = frames > 0 ? this.emulateInWindow / frames : 0;
    this.drawPerFrame = frames > 0 ? this.drawInWindow / frames : 0;
    this.framesPerWake = this.wakesInWindow > 0 ? frames / this.wakesInWindow : 0;

    this.windowStart = now;
    this.framesInWindow = 0;
    this.wakesInWindow = 0;
    this.emulateInWindow = 0;
    this.drawInWindow = 0;
  }

  /** Takes the audio thread's latest report of itself. */
  setAudio(report: AudioReport): void {
    this.audio = report;
  }

  snapshot(): StatsSnapshot {
    const work = this.emulatePerFrame + this.drawPerFrame;
    return {
      fps: this.fps,
      speedPercent: (this.fps / TARGET_HZ) * 100,
      emulate: this.emulatePerFrame,
      draw: this.drawPerFrame,
      work,
      workFraction: work / FRAME_MS,
      framesPerWake: this.framesPerWake,
      wakeRate: this.wakeRate,
      frames: this.frames,
      wakes: this.wakes,
      cappedWakes: this.cappedWakes,
      audio: this.audio
        ? { ...this.audio, bufferedMs: (this.audio.buffered / this.audio.sampleRate) * 1000 }
        : null,
    };
  }
}
