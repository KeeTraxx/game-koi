// Tests for the stats counters and the panel's formatting. Both are pure — the
// counters take the clock as an argument rather than reading it, and the formatters are
// free functions — so none of this needs a browser, a timer or a DOM.
//
// Importing overlay.js is safe here for the same reason: every DOM call in it is inside
// the class, so nothing touches `document` at module scope.

import assert from "node:assert/strict";
import { test } from "node:test";

import { FRAME_MS, FrameStats, TARGET_HZ } from "../dist/stats.js";
import { bufferColour, count, formatMs, formatPercent, loadColour } from "../dist/overlay.js";

/** A wake that emulated `frames` frames, with the given costs in ms. */
function wake(frames, emulate = 0, draw = 0, capped = false) {
  return { emulate, draw, frames, capped };
}

/** Runs `wakes` evenly spaced over `spanMs`, so a sampling window closes. */
function run(stats, wakes, spanMs, report) {
  const step = spanMs / wakes;
  for (let i = 1; i <= wakes; i++) stats.wake(report, i * step);
}

test("nothing is reported until a sampling window closes", () => {
  // A rate computed from one wake is noise; the panel shows zeros rather than a
  // number that would be wrong.
  const stats = new FrameStats(0);
  stats.wake(wake(1, 2, 1), 16);
  assert.equal(stats.snapshot().fps, 0);
});

test("fps counts emulated frames, not wakes", () => {
  // The whole reason this module is not a copy of the desktop's: at 120Hz the loop
  // wakes twice per frame, and counting wakes would report 120 and mean nothing.
  const stats = new FrameStats(0);
  run(stats, 120, 1000, wake(1, 2, 0.5)); // 120 wakes, one frame each
  const snapshot = stats.snapshot();
  assert.equal(Math.round(snapshot.fps), 120);

  // Now the realistic 120Hz case: half the wakes emulate nothing.
  const paced = new FrameStats(0);
  for (let i = 1; i <= 120; i++) {
    paced.wake(wake(i % 2 === 0 ? 1 : 0, 2, 0.5), i * (1000 / 120));
  }
  const pacedSnapshot = paced.snapshot();
  assert.equal(Math.round(pacedSnapshot.fps), 60, "60 frames emulated across 120 wakes");
  assert.ok(Math.abs(pacedSnapshot.framesPerWake - 0.5) < 0.01);
  assert.equal(Math.round(pacedSnapshot.wakeRate), 120, "the display's rate, separately");
});

test("speed is a percentage of the hardware rate", () => {
  // Half of 59.73 Hz should read as 50%, not as 50 fps.
  const stats = new FrameStats(0);
  run(stats, 30, 1000, wake(1, 2, 0.5));
  assert.ok(Math.abs(stats.snapshot().speedPercent - 50.2) < 0.5);
});

test("work is averaged per frame, not per wake", () => {
  // A wake that emulated nothing did no work. Averaging those in would report a
  // machine faster than it is.
  const stats = new FrameStats(0);
  for (let i = 1; i <= 120; i++) {
    const emulated = i % 2 === 0;
    stats.wake(wake(emulated ? 1 : 0, emulated ? 4 : 0, emulated ? 1 : 0), i * (1000 / 120));
  }
  const snapshot = stats.snapshot();
  assert.ok(Math.abs(snapshot.emulate - 4) < 0.01, "4ms per frame, not 2ms per wake");
  assert.ok(Math.abs(snapshot.draw - 1) < 0.01);
  assert.ok(Math.abs(snapshot.work - 5) < 0.01);
  assert.ok(Math.abs(snapshot.workFraction - 5 / FRAME_MS) < 0.001);
});

test("the work fraction crosses 1 when a frame costs more than it is worth", () => {
  const stats = new FrameStats(0);
  run(stats, 60, 1000, wake(1, 20, 2));
  assert.ok(stats.snapshot().workFraction > 1, "22ms of work against a 16.74ms budget");
});

test("capped wakes accumulate across windows", () => {
  const stats = new FrameStats(0);
  run(stats, 60, 1000, wake(4, 8, 1, true));
  assert.equal(stats.snapshot().cappedWakes, 60);
  assert.equal(stats.snapshot().wakes, 60);
  assert.equal(stats.snapshot().frames, 240);
});

test("audio is absent until the worklet reports", () => {
  assert.equal(new FrameStats(0).snapshot().audio, null);
});

test("the audio buffer is converted to milliseconds", () => {
  const stats = new FrameStats(0);
  stats.setAudio({
    buffered: 2400,
    target: 1600,
    underruns: 3,
    dropped: 0,
    sampleRate: 48000,
    state: "running",
  });
  const audio = stats.snapshot().audio;
  // 2400 sample frames at 48kHz is 50ms of sound queued ahead.
  assert.ok(Math.abs(audio.bufferedMs - 50) < 0.001);
  assert.equal(audio.underruns, 3);
});

test("the frame budget matches the hardware rate", () => {
  // 16.74ms, not 16.67: the DMG runs at 59.73Hz.
  assert.ok(Math.abs(FRAME_MS - 16.74) < 0.01);
  assert.ok(Math.abs(TARGET_HZ - 59.73) < 0.01);
});

test("durations are formatted to a fixed width so the column cannot jitter", () => {
  assert.equal(formatMs(1.8), "  1.80 ms");
  assert.equal(formatMs(12.345), " 12.35 ms");
  assert.equal(formatMs(0), "  0.00 ms");
  assert.equal(formatMs(1.8).length, formatMs(123.4).length);
});

test("percentages and counts share a width", () => {
  assert.equal(formatPercent(12.4), "  12%");
  assert.equal(formatPercent(100), " 100%");
  assert.equal(count(0), "    0");
  assert.equal(count(0).length, formatPercent(100).length);
});

test("load colour only reacts near the budget", () => {
  // A figure that matters at 100% is easy to miss if it is coloured at 12%.
  const plain = loadColour(0.12);
  assert.equal(loadColour(0.5), plain);
  assert.notEqual(loadColour(0.9), plain, "approaching the budget is worth noticing");
  assert.notEqual(loadColour(1.5), loadColour(0.9), "over it is worse than near it");
});

test("an empty audio buffer is worse than a shallow one", () => {
  const target = 33;
  const healthy = bufferColour(33, target);
  assert.equal(bufferColour(20, target), healthy, "at the target, nothing to say");
  assert.notEqual(bufferColour(8, target), healthy, "draining below half the target");
  assert.notEqual(bufferColour(0, target), bufferColour(8, target), "dry was audible");
});
