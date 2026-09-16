/**
 * game-koi — a Game Boy (DMG) emulator, compiled to WebAssembly.
 *
 * `GameKoi` is the whole integration surface: point it at a canvas and hand it a
 * ROM's bytes, and it owns wasm setup, the AudioWorklet, and the render/audio pacing
 * loop. Nothing else needs to be served or configured by the page — the wasm binary
 * ships as a normal ES module asset next to this file, and the audio worklet is
 * inlined as a Blob URL.
 */

import init, { Emulator } from "../wasm/game_koi_web.js";
import { WORKLET_SOURCE } from "./worklet.js";
import { DEFAULT_GAMEPAD_MAP, GamepadInput, type Action } from "./gamepad.js";
import { FrameStats } from "./stats.js";
import { StatsOverlay } from "./overlay.js";

export type Button = "up" | "down" | "left" | "right" | "a" | "b" | "start" | "select";

export interface GameKoiOptions {
  /** Canvas the emulator draws into. Resized to the Game Boy's 160x144. */
  canvas: HTMLCanvasElement;
  /** The ROM image — a `.gb` file's bytes. */
  rom: Uint8Array;
  /**
   * Maps `KeyboardEvent.code` to a button. Pass `null` to disable built-in keyboard
   * handling entirely and drive `press`/`release` yourself. Defaults to the desktop
   * build's layout: arrow keys, Z/X, Enter, right Shift.
   */
  keymap?: Record<string, Button> | null;
  /**
   * Maps a standard-layout gamepad's button indices to a button. Pass `null` to
   * disable gamepad polling entirely. Defaults to the desktop build's layout: d-pad,
   * East/South face buttons for A/B, Start and Back/Select. The left analog stick acts
   * as a d-pad regardless of this map, since it is a direction rather than an index.
   */
  gamepadMap?: Record<number, Button> | null;
  /**
   * `KeyboardEvent.code` that toggles the debug stats panel. Default `"Backquote"`
   * (the ` / ~ key). Pass `null` to bind no key and drive {@link GameKoi.toggleOverlay}
   * yourself. Like `keymap`, this is a physical key position, not a character.
   */
  overlayKey?: string | null;
  /** Samples to keep queued ahead of playback. Default 1600 (~2 frames at 48kHz). */
  targetBuffer?: number;
  /**
   * Safety cap on frames emulated per wake-up. Without this, a backgrounded tab
   * (where `requestAnimationFrame` stops firing) would return to a large deficit and
   * freeze the page catching up. Default 4.
   */
  maxFramesPerWake?: number;
}

const DEFAULT_KEYMAP: Record<string, Button> = {
  ArrowRight: "right",
  ArrowLeft: "left",
  ArrowUp: "up",
  ArrowDown: "down",
  KeyZ: "a",
  KeyX: "b",
  Enter: "start",
  ShiftRight: "select",
};

// The wasm module only needs instantiating once per page, no matter how many
// `GameKoi` instances are created.
let wasmReady: ReturnType<typeof init> | null = null;
function ensureWasm() {
  if (!wasmReady) wasmReady = init();
  return wasmReady;
}

/** A running Game Boy, attached to a canvas and driving an AudioWorklet. */
export class GameKoi {
  private readonly ctx: CanvasRenderingContext2D;
  private readonly audioContext: AudioContext;
  private readonly worklet: AudioWorkletNode;
  private readonly wasmMemory: WebAssembly.Memory;
  private readonly imageData: ImageData;
  private readonly keymap: Record<string, Button> | null;
  private readonly gamepad: GamepadInput | null;
  private readonly targetBuffer: number;
  private readonly maxFramesPerWake: number;
  private readonly stats: FrameStats;
  private emulator: Emulator;
  private buffered = 0;
  /** Cumulative worklet counters, as last reported. See `worklet.ts`. */
  private underruns = 0;
  private dropped = 0;
  /** Built on first toggle, so a page that never opens it gets no element. */
  private overlay: StatsOverlay | null = null;
  private running = false;
  private rafHandle = 0;
  private keydownListener?: (event: KeyboardEvent) => void;
  private keyupListener?: (event: KeyboardEvent) => void;
  private overlayListener?: (event: KeyboardEvent) => void;

  private constructor(
    emulator: Emulator,
    wasmMemory: WebAssembly.Memory,
    ctx: CanvasRenderingContext2D,
    audioContext: AudioContext,
    worklet: AudioWorkletNode,
    keymap: Record<string, Button> | null,
    gamepadMap: Record<number, Button> | null,
    overlayKey: string | null,
    targetBuffer: number,
    maxFramesPerWake: number,
  ) {
    this.emulator = emulator;
    this.wasmMemory = wasmMemory;
    this.ctx = ctx;
    this.audioContext = audioContext;
    this.worklet = worklet;
    this.keymap = keymap;
    this.gamepad = gamepadMap ? new GamepadInput(gamepadMap) : null;
    this.targetBuffer = targetBuffer;
    this.maxFramesPerWake = maxFramesPerWake;
    this.imageData = ctx.createImageData(Emulator.width(), Emulator.height());
    this.stats = new FrameStats(performance.now());

    this.worklet.port.onmessage = (event) => {
      this.buffered = event.data.buffered;
      this.underruns = event.data.underruns;
      this.dropped = event.data.dropped;
    };
    if (keymap) this.attachKeyboard();
    if (overlayKey !== null) this.attachOverlayKey(overlayKey);
  }

  /**
   * Builds and starts a running emulator.
   *
   * Must be called from inside a user gesture (a click or keypress handler) —
   * browsers suspend a freshly created `AudioContext` otherwise.
   */
  static async create(options: GameKoiOptions): Promise<GameKoi> {
    const wasm = await ensureWasm();

    const canvas = options.canvas;
    canvas.width = Emulator.width();
    canvas.height = Emulator.height();
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2D context unavailable");

    const audioContext = new AudioContext();
    const blobUrl = URL.createObjectURL(
      new Blob([WORKLET_SOURCE], { type: "application/javascript" }),
    );
    try {
      await audioContext.audioWorklet.addModule(blobUrl);
    } finally {
      URL.revokeObjectURL(blobUrl);
    }
    const worklet = new AudioWorkletNode(audioContext, "koi-processor", {
      outputChannelCount: [2],
    });
    worklet.connect(audioContext.destination);

    const emulator = new Emulator(options.rom, audioContext.sampleRate);
    const keymap = options.keymap === null ? null : options.keymap ?? DEFAULT_KEYMAP;
    const gamepadMap =
      options.gamepadMap === null ? null : options.gamepadMap ?? DEFAULT_GAMEPAD_MAP;

    const koi = new GameKoi(
      emulator,
      wasm.memory,
      ctx,
      audioContext,
      worklet,
      keymap,
      gamepadMap,
      options.overlayKey === null ? null : options.overlayKey ?? "Backquote",
      options.targetBuffer ?? 1600,
      options.maxFramesPerWake ?? 4,
    );
    koi.running = true;
    koi.loop();
    return koi;
  }

  /** Swaps in a new ROM, keeping the same canvas and audio graph. */
  loadRom(rom: Uint8Array): void {
    // The new machine's joypad starts with nothing held, so the snapshot of what is
    // held has to start over too or the first poll will emit no press for a direction
    // that was already down.
    this.gamepad?.releaseAll();
    this.emulator.free();
    this.emulator = new Emulator(rom, this.audioContext.sampleRate);
  }

  press(button: Button): void {
    this.emulator.press(button);
  }

  release(button: Button): void {
    this.emulator.release(button);
  }

  /**
   * Shows or hides the debug stats panel — the same thing the ` key does.
   *
   * The panel is built the first time this is called, so a page that never opens it
   * never gets an extra element in its DOM.
   */
  toggleOverlay(): void {
    this.overlay ??= new StatsOverlay(this.ctx.canvas);
    this.overlay.toggle();
  }

  /** Whether the stats panel is currently showing. */
  get overlayVisible(): boolean {
    return this.overlay?.visible ?? false;
  }

  pause(): void {
    this.running = false;
    cancelAnimationFrame(this.rafHandle);
    // A direction held at the moment of the pause has no poll coming to release it,
    // so it would still be down when the machine resumes.
    this.applyGamepadActions(this.gamepad?.releaseAll());
    void this.audioContext.suspend();
  }

  resume(): void {
    if (this.running) return;
    this.running = true;
    void this.audioContext.resume();
    this.loop();
  }

  /** Stops the loop, releases wasm memory, and tears down the audio graph. */
  dispose(): void {
    this.running = false;
    cancelAnimationFrame(this.rafHandle);
    this.detachKeyboard();
    if (this.overlayListener) removeEventListener("keydown", this.overlayListener);
    this.overlay?.dispose();
    this.emulator.free();
    this.worklet.disconnect();
    void this.audioContext.close();
  }

  private loop = (): void => {
    if (!this.running) return;

    const wokeAt = performance.now();
    this.pollGamepads();
    // Measured from after the input poll: reading a gamepad snapshot is neither
    // emulation nor drawing, and it costs a fraction of a microsecond.
    const emulateStart = performance.now();

    // Emulate however many frames the audio buffer is short by, capped. Audio wins
    // when the display's refresh rate and the Game Boy's 59.73Hz disagree, since a
    // dry buffer is audible and a repeated frame is not.
    let frames = 0;
    while (this.buffered < this.targetBuffer && frames < this.maxFramesPerWake) {
      this.emulator.run_frame();
      const samples = this.emulator.take_samples();
      if (samples.length > 0) {
        const count = samples.length / 2;
        const left = new Float32Array(count);
        const right = new Float32Array(count);
        for (let i = 0; i < count; i++) {
          left[i] = samples[i * 2];
          right[i] = samples[i * 2 + 1];
        }
        // Transfer rather than copy: these are throwaway buffers.
        this.worklet.port.postMessage({ left, right }, [left.buffer, right.buffer]);
        this.buffered += count;
      }
      frames++;
    }

    const drawStart = performance.now();
    if (frames > 0) this.drawFrame();
    const drawEnd = performance.now();

    this.recordWake(wokeAt, emulateStart, drawStart, drawEnd, frames);
    this.rafHandle = requestAnimationFrame(this.loop);
  };

  /**
   * Feeds the counters and refreshes the panel.
   *
   * Always run, not just while the panel is open: the averages are computed over half a
   * second, so a panel that only started counting when it opened would show nothing for
   * its first window. The cost is four `performance.now()` reads per wake.
   */
  private recordWake(
    wokeAt: number,
    emulateStart: number,
    drawStart: number,
    drawEnd: number,
    frames: number,
  ): void {
    this.stats.wake(
      {
        emulate: drawStart - emulateStart,
        draw: frames > 0 ? drawEnd - drawStart : 0,
        frames,
        // The catch-up was clamped and the buffer is still short, so a deficit is
        // being carried into the next wake.
        capped: frames >= this.maxFramesPerWake && this.buffered < this.targetBuffer,
      },
      wokeAt,
    );
    this.stats.setAudio({
      buffered: this.buffered,
      target: this.targetBuffer,
      underruns: this.underruns,
      dropped: this.dropped,
      sampleRate: this.audioContext.sampleRate,
      state: this.audioContext.state,
    });
    this.overlay?.update(this.stats.snapshot(), wokeAt);
  }

  private drawFrame(): void {
    // Read `wasmMemory.buffer` fresh rather than caching it: any wasm allocation can
    // grow linear memory, which allocates a new ArrayBuffer and silently detaches
    // views over the old one. Re-reading it here means a grown buffer is picked up
    // automatically instead of producing a zero-length view.
    const bytes = new Uint8ClampedArray(
      this.wasmMemory.buffer,
      this.emulator.frame_ptr(),
      this.emulator.frame_len(),
    );
    this.imageData.data.set(bytes);
    this.ctx.putImageData(this.imageData, 0, 0);
  }

  /**
   * Reads every connected pad and applies whatever changed since the last wake.
   *
   * Done here rather than from a listener because the Gamepad API fires no event for a
   * button: `getGamepads()` is a snapshot and polling it is the only way to see one.
   */
  private pollGamepads(): void {
    if (!this.gamepad) return;
    // Guarded because the API is absent outside a secure context, and in a few
    // embedded webviews that still have no gamepad support at all.
    const pads = navigator.getGamepads?.();
    if (!pads) return;
    this.applyGamepadActions(this.gamepad.poll(pads));
  }

  private applyGamepadActions(actions: Action[] | undefined): void {
    for (const action of actions ?? []) {
      if (action.type === "press") this.press(action.button);
      else this.release(action.button);
    }
  }

  private attachKeyboard(): void {
    this.keydownListener = (event: KeyboardEvent) => {
      const button = this.keymap?.[event.code];
      if (button) {
        this.press(button);
        event.preventDefault();
      }
    };
    this.keyupListener = (event: KeyboardEvent) => {
      const button = this.keymap?.[event.code];
      if (button) {
        this.release(button);
        event.preventDefault();
      }
    };
    addEventListener("keydown", this.keydownListener);
    addEventListener("keyup", this.keyupListener);
  }

  private detachKeyboard(): void {
    if (this.keydownListener) removeEventListener("keydown", this.keydownListener);
    if (this.keyupListener) removeEventListener("keyup", this.keyupListener);
  }

  /**
   * Binds the panel's toggle key.
   *
   * Its own listener rather than a case inside the keymap one, because the two are
   * independent: a page that drives the joypad itself (`keymap: null`) can still want
   * the panel, and the panel key is not a Game Boy button.
   */
  private attachOverlayKey(code: string): void {
    this.overlayListener = (event: KeyboardEvent) => {
      if (event.code !== code || event.repeat) return;
      this.toggleOverlay();
      event.preventDefault();
    };
    addEventListener("keydown", this.overlayListener);
  }
}
