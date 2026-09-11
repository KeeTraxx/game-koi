// The browser frontend.
//
// The desktop build sleeps until each 16.74 ms deadline and runs a frame. That shape
// does not survive the move to a page: you must not block the main thread, and
// requestAnimationFrame fires at the *display's* rate — 60 Hz, 120 Hz, 144 Hz — which
// is not the Game Boy's 59.73 Hz and never will be.
//
// So the audio buffer paces the emulator instead. rAF is only the wake-up; on each
// wake we ask how much audio the worklet still has and emulate enough frames to top it
// back up. Audio then runs at exactly the rate the sound hardware consumes it, and the
// picture follows. A repeated or dropped frame is nearly invisible; a dry audio buffer
// is instantly audible, so when the two clocks disagree, audio wins.

import init, { Emulator } from "./pkg/game_koi_web.js";

const canvas = document.getElementById("screen");
const ctx = canvas.getContext("2d");
const statusEl = document.getElementById("status");
const fileInput = document.getElementById("rom");

// How much audio to keep queued ahead. Two frames' worth is enough to absorb a late
// wake-up without adding latency anyone would notice.
const TARGET_BUFFER = 1600;
// Never run more than this in one wake-up. Without it, a backgrounded tab (where rAF
// stops firing) would return to a huge deficit and freeze the page catching up.
const MAX_FRAMES_PER_WAKE = 4;

let emulator = null;
let audioContext = null;
let worklet = null;
let buffered = 0;
let imageData = null;
let wasmMemory = null;

// Keyboard layout mirrors the desktop build's.
const KEYS = {
  ArrowRight: "right", ArrowLeft: "left", ArrowUp: "up", ArrowDown: "down",
  KeyZ: "a", KeyX: "b", Enter: "start", ShiftRight: "select",
};

async function startAudio() {
  // Must be created from a user gesture, or the browser suspends it.
  audioContext = new AudioContext();
  await audioContext.audioWorklet.addModule("./audio-worklet.js");
  worklet = new AudioWorkletNode(audioContext, "koi-processor", {
    outputChannelCount: [2],
  });
  worklet.port.onmessage = (event) => {
    buffered = event.data.buffered;
  };
  worklet.connect(audioContext.destination);
  return audioContext.sampleRate;
}

function drawFrame() {
  // The view is rebuilt each frame because growing wasm memory detaches any existing
  // view over its buffer, and that is silent — you get a zero-length array, not an
  // error.
  if (wasmMemory.buffer !== imageData.buffer_ref) {
    imageData = ctx.createImageData(Emulator.width(), Emulator.height());
    imageData.buffer_ref = wasmMemory.buffer;
  }
  const bytes = new Uint8ClampedArray(
    wasmMemory.buffer, emulator.frame_ptr(), emulator.frame_len(),
  );
  imageData.data.set(bytes);
  ctx.putImageData(imageData, 0, 0);
}

function pump() {
  if (!emulator) return;

  // Emulate however many frames the audio buffer is short by, capped.
  let frames = 0;
  while (buffered < TARGET_BUFFER && frames < MAX_FRAMES_PER_WAKE) {
    emulator.run_frame();
    const samples = emulator.take_samples();
    if (samples.length > 0) {
      const count = samples.length / 2;
      const left = new Float32Array(count);
      const right = new Float32Array(count);
      for (let i = 0; i < count; i++) {
        left[i] = samples[i * 2];
        right[i] = samples[i * 2 + 1];
      }
      // Transfer rather than copy: these are throwaway buffers.
      worklet.port.postMessage({ left, right }, [left.buffer, right.buffer]);
      buffered += count;
    }
    frames++;
  }

  if (frames > 0) drawFrame();
  requestAnimationFrame(pump);
}

fileInput.addEventListener("change", async (event) => {
  const file = event.target.files[0];
  if (!file) return;

  try {
    const rom = new Uint8Array(await file.arrayBuffer());
    if (!audioContext) {
      const rate = await startAudio();
      statusEl.textContent = `audio: ${rate} Hz`;
    }
    await audioContext.resume();

    if (emulator) emulator.free();
    emulator = new Emulator(rom, audioContext.sampleRate);

    imageData = ctx.createImageData(Emulator.width(), Emulator.height());
    imageData.buffer_ref = wasmMemory.buffer;
    statusEl.textContent = `running ${file.name} — audio ${audioContext.sampleRate} Hz`;
    requestAnimationFrame(pump);
  } catch (err) {
    // A bad header or an unsupported mapper arrives here as a thrown JsError.
    statusEl.textContent = `error: ${err}`;
    emulator = null;
  }
});

addEventListener("keydown", (event) => {
  const button = KEYS[event.code];
  if (button && emulator) { emulator.press(button); event.preventDefault(); }
});
addEventListener("keyup", (event) => {
  const button = KEYS[event.code];
  if (button && emulator) { emulator.release(button); event.preventDefault(); }
});

const wasm = await init();
wasmMemory = wasm.memory;
statusEl.textContent = "ready — choose a ROM";
