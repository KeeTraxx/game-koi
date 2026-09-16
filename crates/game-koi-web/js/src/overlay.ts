/**
 * The debug stats panel.
 *
 * This is the browser counterpart of `game-koi-desktop`'s `overlay.rs`, and the split is
 * the same on both sides: `stats.ts` counts, this draws. What differs is the medium.
 * The desktop panel is egui, rendered into the same wgpu pass as the game; egui here
 * would mean bringing wgpu or WebGL into a package whose whole rendering path is one
 * `putImageData` call, and roughly 2 MB of wasm paid by every consumer whether or not
 * they ever press the key. So this is a plain DOM panel with the same content.
 *
 * Three consequences of that choice, all deliberate:
 *
 * - **It is a sibling of the canvas, not a wrapper around it.** The element is appended
 *   to `document.body` and positioned over the canvas from its bounding rect. Wrapping
 *   the consumer's canvas in a new element would be the tidier CSS, but it would move
 *   their node in the document and break any selector or layout rule that depended on
 *   where it was.
 * - **`pointer-events: none`.** The panel is read-only, so it must never swallow a click
 *   meant for the page underneath. The desktop panel does have widgets, but the two it
 *   has — vsync and the CRT mode — are host-render settings that do not exist here.
 * - **Styles are inline.** Nothing to serve, nothing for a bundler to find, and no
 *   chance of a page's stylesheet reaching in; the same reasoning that keeps the audio
 *   worklet a Blob rather than a file.
 */

import type { StatsSnapshot } from "./stats.js";

/**
 * How often the text is rewritten.
 *
 * Deliberately slower than the frame rate: the underlying averages only change when a
 * sampling window closes, and text that changes 60 times a second is unreadable anyway.
 * The DOM writes are what this throttle is really for — the measurements carry on every
 * wake regardless.
 */
const REFRESH_MS = 250;

/** Distance from the canvas's top-left corner, matching the desktop panel's [8, 8]. */
const INSET_PX = 8;

const TEXT = "#c8d0d8";
const DIM = "#8896a0";
const WARN = "#e8c060";
const BAD = "#e05050";

const PANEL_STYLE: Partial<CSSStyleDeclaration> = {
  position: "fixed",
  // Above anything a page is plausibly stacking, since a debug panel hidden behind the
  // page's own chrome would be useless.
  zIndex: "2147483647",
  pointerEvents: "none",
  font: "12px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
  color: TEXT,
  background: "rgba(8, 24, 32, 0.86)",
  border: "1px solid rgba(136, 192, 112, 0.4)",
  borderRadius: "4px",
  padding: "6px 8px",
  display: "none",
};

/** A read-only panel of counters, floating over the emulator's canvas. */
export class StatsOverlay {
  private readonly canvas: HTMLCanvasElement;
  private readonly element: HTMLDivElement;
  private readonly grid: HTMLDivElement;
  private readonly values = new Map<string, HTMLSpanElement>();
  private shown = false;
  private lastRefresh = 0;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;

    this.element = document.createElement("div");
    Object.assign(this.element.style, PANEL_STYLE);

    const title = document.createElement("div");
    title.textContent = "Stats";
    title.style.marginBottom = "4px";
    title.style.color = DIM;
    this.element.appendChild(title);

    this.grid = document.createElement("div");
    this.grid.style.display = "grid";
    this.grid.style.gridTemplateColumns = "auto auto";
    this.grid.style.columnGap = "14px";
    this.element.appendChild(this.grid);

    // The same grouping as the desktop panel: how fast, what it cost, how the two
    // clocks relate, and what the audio device is doing.
    this.addRow("FPS");
    this.addRow("Speed");
    this.addSeparator();
    this.addRow("  emulate");
    this.addRow("  draw");
    this.addRow("Work");
    this.addSeparator();
    this.addRow("Frames/wake");
    this.addRow("Refresh");
    this.addRow("Capped");
    this.addSeparator();
    this.addRow("Buffer");
    this.addRow("Underruns");
    this.addRow("Dropped");
    this.addRow("Output");

    document.body.appendChild(this.element);
  }

  get visible(): boolean {
    return this.shown;
  }

  toggle(): void {
    this.shown = !this.shown;
    this.element.style.display = this.shown ? "block" : "none";
    // Force the next update through: the panel should not open showing whatever was
    // last written up to a refresh interval ago.
    this.lastRefresh = 0;
  }

  /**
   * Rewrites the panel, at most every {@link REFRESH_MS}.
   *
   * Cheap to call every frame — while hidden it does nothing at all, which is what keeps
   * a switched-off feature costing a branch rather than a layout.
   */
  update(snapshot: StatsSnapshot, now: number): void {
    if (!this.shown) return;
    if (now - this.lastRefresh < REFRESH_MS) return;
    this.lastRefresh = now;

    this.position();

    this.set("FPS", snapshot.fps.toFixed(1));
    this.set("Speed", formatPercent(snapshot.speedPercent));
    this.set("  emulate", formatMs(snapshot.emulate));
    this.set("  draw", formatMs(snapshot.draw));
    // The headline cost figure, coloured the same way the desktop colours its budget:
    // worth noticing near the limit, noise the rest of the time.
    this.set("Work", formatPercent(snapshot.workFraction * 100), loadColour(snapshot.workFraction));

    this.set("Frames/wake", snapshot.framesPerWake.toFixed(2));
    this.set("Refresh", `${snapshot.wakeRate.toFixed(1)} Hz`);
    // A capped wake means the catch-up was clamped and a deficit was left behind —
    // usually a backgrounded tab, occasionally a machine that cannot keep up.
    this.set("Capped", count(snapshot.cappedWakes), snapshot.cappedWakes > 0 ? WARN : TEXT);

    const audio = snapshot.audio;
    if (!audio) {
      // Only reachable in the first moments, before the worklet's first report.
      this.set("Buffer", "—");
      this.set("Underruns", "—");
      this.set("Dropped", "—");
      this.set("Output", "—");
      return;
    }

    const targetMs = (audio.target / audio.sampleRate) * 1000;
    this.set(
      "Buffer",
      `${audio.bufferedMs.toFixed(1)} ms`,
      bufferColour(audio.bufferedMs, targetMs),
    );
    // Cumulative, and the closest thing here to the desktop's late-frame count: an
    // underrun is a moment of silence that was actually heard.
    this.set("Underruns", count(audio.underruns), audio.underruns > 0 ? WARN : TEXT);
    // The opposite failure — the page ran ahead of the sound card and the ring threw
    // samples away. Normal in small numbers after a tab comes back to the foreground.
    this.set("Dropped", count(audio.dropped), audio.dropped > 0 ? WARN : TEXT);
    this.set(
      "Output",
      `${(audio.sampleRate / 1000).toFixed(1)}k ${audio.state}`,
      // A suspended context is the first thing to check when there is no sound, and
      // nothing else on the panel would say so.
      audio.state === "running" ? TEXT : WARN,
    );
  }

  dispose(): void {
    this.element.remove();
  }

  /**
   * Anchors the panel to the canvas's current position.
   *
   * Re-read rather than cached because a page can scroll, resize or reflow under us with
   * no event this module sees. `getBoundingClientRect` forces layout, which is why it
   * happens on the refresh tick and only while the panel is open.
   */
  private position(): void {
    const rect = this.canvas.getBoundingClientRect();
    this.element.style.left = `${rect.left + INSET_PX}px`;
    this.element.style.top = `${rect.top + INSET_PX}px`;
  }

  private addRow(label: string): void {
    const labelCell = document.createElement("span");
    labelCell.textContent = label;
    labelCell.style.color = DIM;
    // Leading spaces mark the sub-rows of the work breakdown, as they do on the
    // desktop; without this the browser collapses them.
    labelCell.style.whiteSpace = "pre";

    const valueCell = document.createElement("span");
    valueCell.textContent = "—";
    valueCell.style.textAlign = "right";
    valueCell.style.whiteSpace = "pre";

    this.grid.appendChild(labelCell);
    this.grid.appendChild(valueCell);
    this.values.set(label, valueCell);
  }

  /**
   * A rule across both columns.
   *
   * `grid-column: 1 / -1` rather than one cell per column: two cells would be broken in
   * the middle by the column gap, which reads as two dashes rather than the single line
   * egui draws.
   */
  private addSeparator(): void {
    const rule = document.createElement("div");
    rule.style.gridColumn = "1 / -1";
    rule.style.borderTop = "1px solid rgba(200, 208, 216, 0.15)";
    rule.style.margin = "3px 0";
    this.grid.appendChild(rule);
  }

  private set(label: string, text: string, colour: string = TEXT): void {
    const cell = this.values.get(label);
    if (!cell) return;
    cell.textContent = text;
    cell.style.color = colour;
  }
}

/**
 * A duration in milliseconds at a fixed width, so the column does not jitter.
 *
 * The desktop formats these as `{:>6.2} ms` for exactly the same reason.
 */
export function formatMs(ms: number): string {
  return `${ms.toFixed(2).padStart(6)} ms`;
}

export function formatPercent(percent: number): string {
  return `${percent.toFixed(0).padStart(4)}%`;
}

/** A plain count, right-aligned to the same width as the percentages above it. */
export function count(value: number): string {
  return value.toString().padStart(5);
}

/**
 * Green through to red as a figure approaches and passes its budget.
 *
 * Only worth colouring near the limit: a number that matters at 100% is easy to miss if
 * it spends most of its time at 12%.
 */
export function loadColour(fraction: number): string {
  if (fraction > 1) return BAD;
  if (fraction > 0.8) return WARN;
  return TEXT;
}

/**
 * How alarming the audio buffer's depth is.
 *
 * Zero is not a warning but a failure that was audible: the worklet emitted silence.
 * Below half the target is the warning, because that is the buffer draining rather than
 * sitting where the pacing loop means to hold it.
 */
export function bufferColour(bufferedMs: number, targetMs: number): string {
  if (bufferedMs <= 0) return BAD;
  if (bufferedMs < targetMs / 2) return WARN;
  return TEXT;
}
