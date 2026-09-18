# game-koi

A Game Boy (DMG) emulator, compiled to WebAssembly, wrapped in a small TypeScript API
that owns the canvas rendering, audio, and pacing loop for you.

```
npm install game-koi
```

## Usage

```ts
import { GameKoi } from "game-koi";

const canvas = document.querySelector("canvas")!;
const romBytes = new Uint8Array(await (await fetch("/game.gb")).arrayBuffer());

// Call from inside a user gesture (e.g. a click handler) — browsers suspend a
// freshly created AudioContext otherwise.
button.addEventListener("click", async () => {
  const koi = await GameKoi.create({ canvas, rom: romBytes });

  // koi.pause(); koi.resume(); koi.dispose();
  // koi.press("a"); koi.release("a");
});
```

That's the whole integration: no separate files to serve for the audio worklet, no
manual wasm memory or canvas bookkeeping, and both input devices are wired up
automatically — the keyboard layout (arrow keys, Z/X, Enter, right Shift) and any
connected gamepad. Pass `keymap: null` or `gamepadMap: null` to `create()` to opt out
of either and drive `press`/`release` yourself — from touch controls or your own
bindings.

### Gamepads

A standard-layout pad maps d-pad to d-pad, the East and South face buttons to A and B
(the DMG's diagonal), Start to Start and Back/Select to Select. The left analog stick
doubles as the d-pad, thresholded at half deflection. Every connected pad drives the
one player, since the Game Boy has only one.

Two things follow from how the browser exposes pads, and neither needs anything from
the page:

- There is no event for a button being pressed — only connect/disconnect — so pads are
  polled once per animation frame. That is also why a pad shows up on its own in
  Chrome, which hides pads until one has been interacted with.
- Pads the browser cannot identify (`mapping !== "standard"`) are ignored, because the
  button indices of an unrecognised layout are whatever the driver enumerated and
  binding them would be confidently wrong rather than merely absent. Use
  `gamepadMap: null` and your own polling for such a pad.

### Button events

`koi.on("press" | "release", listener)` reports button edges from every input path —
the built-in keyboard and gamepad handling as well as your own `press`/`release` calls —
so an on-screen joypad can mirror whatever is driving the emulator:

```ts
const stop = koi.on("press", ({ button, source, pressed }) => {
  // button: the one that changed. source: "keyboard" | "gamepad" | "api".
  // pressed: everything held after this edge, as a Set — render straight from it.
  render(pressed);
});
koi.on("release", ({ pressed }) => render(pressed));

stop(); // or koi.off("press", listener)
```

Only *changes* are reported. A key held down with autorepeat, a stick resting past the
threshold, and `press("a")` called twice each produce one `press` event and nothing more
until the button comes up — so a listener never has to de-duplicate. `koi.pressed` gives
the same set on demand, for a display built after the fact.

Two cases release without anyone letting go: `loadRom()` releases everything (as
`"api"`), since the new machine's joypad starts empty, and `pause()` releases whatever
the gamepad was holding (as `"gamepad"`), since polling stops and nothing else would.
Keys keep their own `keyup` while paused. `dispose()` drops every listener.

### Debug overlay

Press <kbd>`</kbd> (backtick) to toggle a stats panel over the canvas, or call
`koi.toggleOverlay()`. It is the browser counterpart of the desktop build's panel, on
the same key, and it counts browser-shaped things:

- **FPS / Speed** — emulated frames per second, and that as a percentage of a real
  DMG's 59.73 Hz. Frames, not animation-frame wakes: on a 120 Hz display the loop wakes
  twice per frame.
- **emulate / draw / Work** — milliseconds per emulated frame, and their total as a
  share of the 16.74 ms a frame is worth.
- **Frames/wake, Refresh, Capped** — how the two clocks relate, and how often
  `maxFramesPerWake` clamped a catch-up.
- **Buffer / Underruns / Dropped / Output** — how much audio is queued ahead, and the
  two ways that goes wrong. Underruns are silence that was actually heard, and are the
  closest thing here to the desktop's "late frames"; a `suspended` output is the usual
  reason for no sound at all.

The panel is read-only and `pointer-events: none`, so it never intercepts a click, and
it is built the first time it is opened — a page that never opens it gets no extra
element. Pass `overlayKey: null` to bind no key.

## API

- `GameKoi.create(options)` — builds and starts a running emulator.
  - `canvas: HTMLCanvasElement` — resized to 160x144 and drawn into every frame.
  - `rom: Uint8Array` — the `.gb` file's bytes.
  - `keymap?: Record<string, Button> | null` — maps `KeyboardEvent.code` to a
    button; `null` disables built-in keyboard handling.
  - `gamepadMap?: Record<number, Button> | null` — maps a standard-layout gamepad's
    button indices to a button; `null` disables gamepad polling. The left stick acts
    as a d-pad regardless of this map.
  - `overlayKey?: string | null` — `KeyboardEvent.code` toggling the stats panel.
    Default `"Backquote"`; `null` binds no key.
  - `targetBuffer?: number` — audio samples to keep queued ahead. Default `1600`.
  - `maxFramesPerWake?: number` — cap on frames emulated per wake-up, so a
    backgrounded tab can't return to a freeze. Default `4`.
- `koi.loadRom(rom)` — swaps in a new ROM, keeping the same canvas and audio setup.
- `koi.press(button)` / `koi.release(button)` — `Button` is one of `"up"`, `"down"`,
  `"left"`, `"right"`, `"a"`, `"b"`, `"start"`, `"select"`.
- `koi.on(type, listener)` / `koi.off(type, listener)` — `"press"` and `"release"`
  button edges, from any input path. `on` returns a function that unsubscribes. The
  payload is `{ button, source: "keyboard" | "gamepad" | "api", pressed: Set<Button> }`.
- `koi.pressed` — everything held right now, as a `Set<Button>` snapshot.
- `koi.toggleOverlay()` / `koi.overlayVisible` — the debug stats panel.
- `koi.pause()` / `koi.resume()`.
- `koi.dispose()` — stops the loop and tears down the audio graph. Call this before
  dropping a `GameKoi` instance, or the AudioContext and its worklet leak.

## Notes

- Ships as an ES module with bundled `.d.ts` types; works with any bundler that
  understands wasm as a fetchable asset (Vite, webpack 5, esbuild, Next.js) as well
  as directly in a browser via `<script type="module">`, served over `http(s)://`
  (not `file://` — ES modules and `AudioWorklet` both require an origin).
- This package is the browser build of
  [game-koi](https://github.com/KeeTraxx/game-koi), a from-scratch Game Boy emulator
  written in Rust. The core emulation has no audio/video device of its own; this
  package is what connects it to a web page.
