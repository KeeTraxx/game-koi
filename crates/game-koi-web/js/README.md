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
manual wasm memory or canvas bookkeeping, and the built-in keyboard layout (arrow
keys, Z/X, Enter, right Shift) is wired up automatically. Pass `keymap: null` to
`create()` to opt out of that and drive `press`/`release` yourself — from a gamepad,
touch controls, or your own key bindings.

## API

- `GameKoi.create(options)` — builds and starts a running emulator.
  - `canvas: HTMLCanvasElement` — resized to 160x144 and drawn into every frame.
  - `rom: Uint8Array` — the `.gb` file's bytes.
  - `keymap?: Record<string, Button> | null` — maps `KeyboardEvent.code` to a
    button; `null` disables built-in keyboard handling.
  - `targetBuffer?: number` — audio samples to keep queued ahead. Default `1600`.
  - `maxFramesPerWake?: number` — cap on frames emulated per wake-up, so a
    backgrounded tab can't return to a freeze. Default `4`.
- `koi.loadRom(rom)` — swaps in a new ROM, keeping the same canvas and audio setup.
- `koi.press(button)` / `koi.release(button)` — `Button` is one of `"up"`, `"down"`,
  `"left"`, `"right"`, `"a"`, `"b"`, `"start"`, `"select"`.
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
