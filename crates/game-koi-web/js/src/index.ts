/**
 * game-koi — a Game Boy (DMG) emulator, compiled to WebAssembly.
 *
 * `GameKoi` is the whole integration surface: point it at a canvas and hand it a
 * ROM's bytes, and it owns wasm setup, the AudioWorklet, and the render/audio pacing
 * loop. Nothing else needs to be served or configured by the page — the wasm binary
 * ships as a normal ES module asset next to this file, and the audio worklet is
 * inlined as a Blob URL.
 */

import init, { Emulator, version as wasmVersion } from "../wasm/game_koi_web.js";
import { WORKLET_SOURCE } from "./worklet.js";
import { DEFAULT_GAMEPAD_MAP, GamepadInput, type Action } from "./gamepad.js";
import { Emitter, type ButtonSource, type GameKoiEvents } from "./events.js";
import { FrameStats } from "./stats.js";
import { StatsOverlay } from "./overlay.js";
import { LocalStorageSaves, defaultStorage, saveKey } from "./saves.js";

export type {
  ButtonEvent,
  ButtonSource,
  GameKoiEvents,
  Listener,
  Unsubscribe,
} from "./events.js";

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
  /**
   * Keep battery-backed save RAM in `localStorage`, so a game's saves survive a
   * reload. Default `true`. Cartridges without a battery are never saved, as on
   * hardware. Pass `false` to manage saves yourself with {@link GameKoi.exportSave}
   * and {@link GameKoi.importSave}.
   */
  saves?: boolean;
}

/**
 * The fallback autosave interval, in emulated frames — about thirty seconds, the
 * desktop's too.
 *
 * Most saves are stored the moment the game finishes one: it shuts its RAM gate after
 * writing, which `save_committed` reports. This timer is for games that never shut it —
 * Super Mario Land 2 opens it at boot and uses the RAM as working memory, so it is
 * dirty nearly every frame — and bounds what a crash can lose for them. Leaving the
 * page, `pause`, `loadRom` and `dispose` all store as well, so a normal exit loses
 * nothing either way.
 */
const AUTOSAVE_FRAMES = 1800;

/**
 * The shortest gap between two stores, in emulated frames — about a second. Guards
 * against a game that shuts its gate around every access, which would otherwise store
 * every frame. A commit that arrives sooner waits for this rather than being dropped.
 */
const MIN_SAVE_GAP_FRAMES = 60;

/** What prompted a store, for the console line. */
type SaveReason = "game saved" | "periodic" | "paused" | "page hidden" | "ROM changed" | "disposed";

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
// `GameKoi` instances are created. The version line is logged here for the same
// reason: once per page, not once per instance or per `loadRom`.
let wasmReady: ReturnType<typeof init> | null = null;
function ensureWasm() {
  if (!wasmReady) {
    wasmReady = init().then((wasm) => {
      console.info(`game-koi ${wasmVersion()}`);
      return wasm;
    });
  }
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
  private readonly emitter = new Emitter<GameKoiEvents>();
  /** What the joypad currently has down, whichever device put it there. */
  private readonly held = new Set<Button>();
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
  /** Where saves go, or `null` if saving is off or storage is unavailable. */
  private readonly saves: LocalStorageSaves | null;
  /** The current ROM's storage key. */
  private saveKey = "";
  /** Emulated frames since the save was last stored or checked. */
  private framesSinceSave = 0;
  private flushListener?: () => void;

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
    saves: LocalStorageSaves | null,
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
    this.saves = saves;

    this.worklet.port.onmessage = (event) => {
      this.buffered = event.data.buffered;
      this.underruns = event.data.underruns;
      this.dropped = event.data.dropped;
    };
    if (keymap) this.attachKeyboard();
    if (overlayKey !== null) this.attachOverlayKey(overlayKey);
    if (saves) this.attachSaveFlush();
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

    const storage = options.saves === false ? null : defaultStorage();

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
      storage ? new LocalStorageSaves(storage) : null,
    );
    koi.restoreSave(options.rom);
    koi.running = true;
    koi.loop();
    return koi;
  }

  /** Swaps in a new ROM, keeping the same canvas and audio graph. */
  loadRom(rom: Uint8Array): void {
    // The new machine's joypad starts with nothing held, so what this side believes is
    // held has to start over too — both the gamepad's snapshot (or the first poll would
    // emit no press for a direction that was already down) and the set the events are
    // edges against (or a button "already held" would never be pressed on the new
    // machine, and a display would show it stuck down).
    this.gamepad?.releaseAll();
    this.releaseAll();
    // Before the old machine is freed, or whatever it saved since the last autosave
    // goes with it.
    this.flushSave("ROM changed");
    this.emulator.free();
    this.emulator = new Emulator(rom, this.audioContext.sampleRate);
    this.restoreSave(rom);
  }

  /**
   * The cartridge's save RAM, or `null` if it has no battery-backed RAM.
   *
   * The same raw bytes the desktop build writes to its `.sav` file, so either can be
   * handed to the other — and to most other emulators, which use the same format.
   */
  exportSave(): Uint8Array | null {
    return this.emulator.save_data() ?? null;
  }

  /**
   * Replaces the cartridge's save RAM, and stores it if saving is on.
   *
   * Returns `false` if the cartridge has no battery-backed RAM or the data is shorter
   * than its RAM (more likely another game's save than a truncated one of this game's).
   * The running game will not notice until it next reads its save — for most games,
   * that means going back to the title screen or calling {@link loadRom} again.
   */
  importSave(data: Uint8Array): boolean {
    if (!this.emulator.load_save(data)) return false;
    try {
      this.saves?.store(this.saveKey, data);
    } catch (error) {
      console.warn("game-koi: could not store imported save", error);
    }
    return true;
  }

  press(button: Button): void {
    this.setButton(button, true, "api");
  }

  release(button: Button): void {
    this.setButton(button, false, "api");
  }

  /** The version of the emulator core running — the same number logged at startup. */
  get version(): string {
    return wasmVersion();
  }

  /** Everything the joypad has down right now — a snapshot, safe to keep. */
  get pressed(): ReadonlySet<Button> {
    return new Set(this.held);
  }

  /**
   * Listens for button edges, from any input path — the built-in keyboard and gamepad
   * handling as well as `press`/`release` calls.
   *
   * Returns a function that removes the listener again:
   *
   * ```ts
   * const stop = koi.on("press", ({ button, pressed }) => render(pressed));
   * koi.on("release", ({ button, pressed }) => render(pressed));
   * ```
   *
   * Only *changes* are reported: a key held down with autorepeat, a stick resting past
   * the threshold, or `press("a")` twice in a row each produce one `press` event and no
   * `release` until the button actually comes up.
   */
  on<K extends keyof GameKoiEvents>(
    type: K,
    listener: (event: GameKoiEvents[K]) => void,
  ): () => void {
    return this.emitter.on(type, listener);
  }

  /** Removes a listener registered with {@link on}. */
  off<K extends keyof GameKoiEvents>(
    type: K,
    listener: (event: GameKoiEvents[K]) => void,
  ): void {
    this.emitter.off(type, listener);
  }

  /**
   * The one path to the emulated joypad, so every device's edges are counted once.
   *
   * A repeat is dropped here rather than forwarded: `Joypad::press` is idempotent, so
   * skipping it changes nothing for the machine, and it is what makes the events edges
   * instead of a restatement of whatever the browser felt like repeating.
   */
  private setButton(button: Button, pressed: boolean, source: ButtonSource): void {
    if (this.held.has(button) === pressed) return;

    if (pressed) {
      this.held.add(button);
      this.emulator.press(button);
    } else {
      this.held.delete(button);
      this.emulator.release(button);
    }
    this.emitter.emit(pressed ? "press" : "release", {
      button,
      source,
      pressed: new Set(this.held),
    });
  }

  /** Lets go of everything held, emitting the releases. */
  private releaseAll(): void {
    for (const button of [...this.held]) this.setButton(button, false, "api");
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
    this.flushSave("paused");
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
    this.flushSave("disposed");
    this.detachSaveFlush();
    this.detachKeyboard();
    if (this.overlayListener) removeEventListener("keydown", this.overlayListener);
    this.emitter.clear();
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

    this.framesSinceSave += frames;
    if (this.saves && this.framesSinceSave >= MIN_SAVE_GAP_FRAMES) {
      if (this.emulator.save_committed()) this.flushSave("game saved");
      else if (this.framesSinceSave >= AUTOSAVE_FRAMES) this.flushSave("periodic");
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
      this.setButton(action.button, action.type === "press", "gamepad");
    }
  }

  /** Loads the stored save for `rom` into the machine just built for it. */
  private restoreSave(rom: Uint8Array): void {
    this.framesSinceSave = 0;
    if (!this.saves) return;
    this.saveKey = saveKey(rom);
    if (!this.emulator.has_save()) return;
    const data = this.saves.load(this.saveKey);
    // No stored save is the normal first run, and not worth a line.
    if (!data) return;
    const where = `localStorage["${this.saveKey}"]`;
    if (this.emulator.load_save(data)) {
      console.info(`game-koi: restored ${data.length} bytes from ${where}`);
    } else {
      console.warn(`game-koi: ignoring ${where}, it is smaller than this cartridge's RAM`);
    }
  }

  /**
   * Writes the save if the game has touched its RAM since the last write.
   *
   * The dirty flag is cleared only after the write succeeds, so a full quota or
   * blocked storage leaves the save pending for the next attempt rather than
   * silently dropped.
   */
  private flushSave(reason: SaveReason): void {
    this.framesSinceSave = 0;
    if (!this.saves || !this.emulator.save_dirty()) return;
    const data = this.emulator.save_data();
    if (!data) return;
    try {
      this.saves.store(this.saveKey, data);
      this.emulator.mark_saved();
      // Only reached when the game wrote its save RAM, so there is no line for a check
      // that found nothing to do.
      console.info(
        `game-koi: saved ${data.length} bytes to localStorage["${this.saveKey}"] (${reason})`,
      );
    } catch (error) {
      console.warn("game-koi: could not store save", error);
    }
  }

  /**
   * Flushes when the page goes away.
   *
   * `pagehide` and a hidden `visibilitychange` rather than `beforeunload`, which
   * mobile browsers skip when they kill a backgrounded tab. Hidden is the last moment
   * a page is reliably given, and `localStorage` being synchronous is what makes
   * writing in it safe — an async store could be cut off mid-write.
   */
  private attachSaveFlush(): void {
    // Unconditional, since a flush with nothing dirty is a no-op: becoming *visible*
    // costs one check, and `pagehide` need not trust the visibility state to be set yet.
    this.flushListener = () => this.flushSave("page hidden");
    addEventListener("pagehide", this.flushListener);
    document.addEventListener("visibilitychange", this.flushListener);
  }

  private detachSaveFlush(): void {
    if (!this.flushListener) return;
    removeEventListener("pagehide", this.flushListener);
    document.removeEventListener("visibilitychange", this.flushListener);
  }

  private attachKeyboard(): void {
    this.keydownListener = (event: KeyboardEvent) => {
      const button = this.keymap?.[event.code];
      if (button) {
        // Autorepeat still arrives here: `preventDefault` has to run on every repeat,
        // and `setButton` is what collapses them into the one press edge.
        this.setButton(button, true, "keyboard");
        event.preventDefault();
      }
    };
    this.keyupListener = (event: KeyboardEvent) => {
      const button = this.keymap?.[event.code];
      if (button) {
        this.setButton(button, false, "keyboard");
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
