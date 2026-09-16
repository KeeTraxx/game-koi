# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project intent

A Game Boy (DMG) emulator written in Rust, built **as a learning exercise, step by step**. The user is
learning both emulator development and Rust here.

This shapes how to work in this repo:

- Build up in small, working increments. Prefer one subsystem (or one piece of one) at a time, ending at
  a state that compiles and runs, over large multi-subsystem drops.
- Explain the *hardware* reasoning behind code, not just the Rust. Why a flag is set the way it is, why a
  read has a side effect, why a cycle count matters — these are the point of the exercise.
- Don't reach for a crate that does the emulation work (an existing GB core, a CPU instruction-table
  generator). Writing those by hand is the exercise. Crates for the *host* side — windowing, audio out,
  input, error handling, CLI parsing — are fine.
- When introducing an unfamiliar Rust construct, say what it's doing. Keep the code readable over clever.

## State of the repo

**The repo is a Cargo workspace of three crates.** The split is along the one seam the
project already had — the emulated machine knows nothing of the host — so it is a
relocation, not a redesign:

- `crates/game-koi-core/` — the emulated Game Boy. No `std::fs`, no `std::time`, no
  threads. This is what makes the browser build possible, and it **compiles for
  `wasm32-unknown-unknown` unchanged**; keep it that way. Paths below written as
  `src/cpu/` mean `crates/game-koi-core/src/cpu/`.
- `crates/game-koi-desktop/` — the binary (still named `game-koi`), the winit/pixels/
  cpal/gilrs/egui frontend, and battery-backed saves. Everything host-shaped.
- `crates/game-koi-web/` — a `wasm-bindgen` wrapper exposing a low-level `Emulator` to
  JavaScript (`src/lib.rs`), a hand-written TypeScript wrapper around it that is the
  actual **`game-koi` npm package** (`js/`), and a demo page (`index.html` + `main.js`
  at the crate root, so that serving the crate puts the demo at `/`) that consumes
  that package rather than the raw bindings — so the demo doubles as an integration
  test of the thing consumers actually install. Build everything with
  `crates/game-koi-web/build.sh`; the generated `js/wasm/` (wasm-bindgen's output) and
  `js/dist/` (the compiled npm package) are both gitignored.

  **The npm package (`js/`) is the integration surface, not the raw bindings.**
  `js/src/index.ts` exports one class, `GameKoi`: give it a canvas and a ROM's bytes
  and it owns wasm setup, the `AudioContext`/`AudioWorklet`, the render/audio pacing
  loop, and (optionally) keyboard input. This exists because the raw `wasm-bindgen`
  `Emulator` is deliberately low-level (see `src/lib.rs`'s doc comment) — pointers into
  wasm memory, a bare `run_frame`/`take_samples` pair, no pacing — and asking every
  consumer to reimplement the pacing loop and audio wiring correctly would make
  "integrate the emulator" and "reimplement half its host glue" the same task. The
  package ships with TypeScript's own generated `.d.ts` (`js/tsconfig.json` just runs
  `tsc`, no bundler), and `wasm-bindgen --target web` (not `--no-typescript`) so its
  own `.d.ts` carries through too.

  **The `AudioWorklet` processor is a template string (`js/src/worklet.ts`), not a
  file the consumer serves.** `GameKoi.create` turns it into a `Blob` and loads it via
  `URL.createObjectURL` — a page that installs `game-koi` never needs to know the
  worklet exists, let alone configure a server or bundler to find it.

  **Input is keyboard plus gamepad, and the two arrive by opposite means.** The
  keyboard is event-driven (`keydown`/`keyup` listeners in `js/src/index.ts`); the
  Gamepad API fires no event for a button at all, only connect/disconnect, so
  `js/src/gamepad.ts` takes a `navigator.getGamepads()` *snapshot* once per `rAF` wake
  and diffs it against the previous one to recover the press/release edges the joypad
  wants. That polling is also what makes a pad appear in Chrome, which hides pads until
  one is touched. The mapping decisions mirror `game-koi-desktop`'s gilrs code — East/
  South face buttons for A/B, left stick thresholded at half deflection, every
  connected pad driving the one player — with two browser-specific differences: the
  Gamepad API reports **+Y as down** (gilrs reports +Y as up), and pads whose `mapping`
  is not `"standard"` are skipped entirely, since an unrecognised layout's indices are
  whatever the driver enumerated. `gamepad.ts` is pure — snapshots in, edges out — and
  is the one part of the package with unit tests: `js/test/*.test.js`, plain JS against
  the compiled `dist/` so there is no test-only toolchain and nothing test-shaped in the
  published package. `npm test` (or `just test-web`) builds and runs them; CI runs them
  before publishing.

  **The stats overlay is a DOM panel, not egui** (`js/src/overlay.ts` +
  `js/src/stats.ts`, toggled with **`** — the same key the desktop uses). The desktop's
  `overlay.rs` cannot be reused: it is bound to `egui_wgpu` (it borrows the
  `wgpu::Device` `pixels` owns) and `egui_winit`, while this package renders with canvas
  2D `putImageData` and has no winit; `stats.rs` additionally uses `std::time::Instant`,
  which does not work on `wasm32-unknown-unknown`. Bringing real egui over would mean
  WebGL/wgpu plus roughly 2 MB of wasm on an 86 KB package, paid by every consumer
  whether or not they open the panel — so the *content* was ported and the rendering was
  not. The file split mirrors the desktop's anyway (`stats.ts` counts, `overlay.ts`
  draws), which is what keeps the counters unit-testable.

  What it counts differs from the desktop on purpose, because the two are paced
  differently. **`fps` counts emulated frames, not `rAF` wakes** — at 120 Hz the loop
  wakes twice per frame, so counting wakes would report the refresh rate and say nothing
  about the emulator; `framesPerWake` is the ratio between the two clocks. There is **no
  "blocked" figure**, since a page never sleeps and never presents: its replacement is
  the audio buffer, because with pacing driven by the sound card "am I keeping up" *is*
  "is the buffer staying full". The worklet therefore counts underruns (silence actually
  emitted — the closest analogue to the desktop's late frames) and dropped samples, and
  reports both alongside `buffered` in the message it already sends every block. Vsync,
  CRT mode and the GPU/adapter section have no web meaning and are gone; the panel is
  read-only and `pointer-events: none` so it cannot swallow a click.

  Two traps found the hard way: `js/src/worklet.ts` is a **template literal**, so a
  backtick or `${` anywhere in it — including in a comment — ends the string and
  produces a parse error pointing at a line that looks fine. And the panel is a sibling
  of the canvas positioned from its bounding rect, **not** a wrapper around it, since
  wrapping a consumer's canvas would move their node and break their own CSS.

  **The page paces off the audio buffer, not the clock or the display.** This is the
  one place the browser frontend is a different design rather than a translation: the
  desktop build sleeps until each 16.74 ms deadline, but a page must not block its main
  thread, and `requestAnimationFrame` fires at the display's rate — 120 Hz on this
  machine — which is not 59.73 Hz. So `rAF` is only a wake-up; on each wake `GameKoi`
  asks the `AudioWorklet` how much audio is left and emulates enough frames to top it
  up. When the two clocks disagree, audio wins, because a dry buffer is audible and a
  repeated frame is not. `maxFramesPerWake` caps the catch-up so a backgrounded tab
  (where `rAF` stops firing) does not return to a freeze.

  **`wasm-bindgen` the CLI and `wasm-bindgen` the crate must be the same version.**
  Pinned to 0.2.128 in both `Cargo.toml` and `build.sh`, which checks and refuses rather
  than emitting glue that does not match the module's ABI. Install with
  `cargo install wasm-bindgen-cli --version 0.2.128`.

  **Any wasm allocation can detach a JS view over wasm memory**, silently — you get a
  zero-length array, not an error. `GameKoi`'s `drawFrame` re-reads `wasmMemory.buffer`
  fresh every call rather than caching it, specifically so a memory growth mid-session
  is picked up automatically instead of handing back a detached view; do not hoist that
  read out of the per-frame path.

  **`js/package.json`'s version is never hand-edited.** `Cargo.toml`'s
  `workspace.package.version` is the one canonical version number for the whole
  project (Rust crates and the npm package alike); `build.sh` runs `npm pkg set
  version=...` from it on every build, so bumping a release is a single edit in
  `Cargo.toml` followed by `just tag` (which reads that version back out via `cargo
  pkgid` to name and push the release tag).

Check the core still builds for the browser after touching it:

```
cargo build -p game-koi-core --target wasm32-unknown-unknown
```

Two things the split needed, both worth knowing before they bite:

- `header::write_logo_at` is behind the core's **`test-support` feature**, not
  `#[cfg(test)]`, because `cfg(test)` only holds within the crate being compiled and the
  desktop crate's save tests need it. The desktop crate turns the feature on as a
  dev-dependency.
- The ROM suites resolve `test-roms/` via `CARGO_MANIFEST_DIR` **plus `../..`**, since
  that variable now points at the crate rather than the repo root. Get this wrong and
  every ROM test skips silently and the suite still reports green — exactly the failure
  the Testing section warns about.


All seven steps of the build order below are done: the emulator opens a window, plays banked commercial
ROMs, and makes sound. What remains is accuracy work and the gaps listed per subsystem, not missing
subsystems. Verify against the actual tree before assuming anything here is still current.

- `src/cartridge/` — ROM loading and header parsing. Mappers live in `mbc/`, below.
- `src/bus.rs` — the `Bus` trait (`read`/`write`/`tick`/`pending_interrupt`/`acknowledge_interrupt`).
  Implementations are `FlatMemory` (CPU unit tests) and `TestBus`. Interrupts flow CPU-asks-bus, never
  bus-calls-CPU — keep it that way.
- `src/cpu/` — the SM83 core. All 256 opcodes plus the 256-entry `CB` page decode and execute, with
  per-instruction cycle counts, plus interrupt dispatch and the HALT bug.
- `src/interrupts.rs` — the `IF`/`IE` pair and the five sources, in priority order.
- `src/timer.rs` — DIV/TIMA/TMA/TAC, built on the 16-bit counter and falling-edge detector that the
  hardware actually uses, so the DIV-write and TAC-change quirks fall out rather than being special-cased.
- `src/serial.rs` — SB/SC. No peer is attached, so transfers shift in 0xFF; its real job is capturing the
  bytes test ROMs print. External-clock transfers never complete, as on hardware.
- `src/ppu/` — the PPU. Scanline state machine, background, window, sprites, VRAM/OAM mode locking.
  `fetch.rs` holds the rendering; `mod.rs` the timing and registers.
- `src/apu/` — the APU. `mod.rs` holds the registers, the DIV-APU sequencer, the mixer and the resampler;
  `square.rs` (CH1+CH2, sweep included), `wave.rs` (CH3), `noise.rs` (CH4), and `units.rs` for the length
  timer and envelope both share. The 512 Hz sequencer is **DIV bit 4 falling**, not a clock of its own,
  so a `DIV` write can step it early — that falls out rather than being special-cased. The APU emits
  finished stereo samples into a bounded queue; the core never talks to an audio device.
- `game-koi-desktop/src/save.rs` — battery-backed saves. Only for cartridges whose header says they have a battery: RAM on
  a battery-less board is volatile on hardware too, so persisting it would invent a memory the cartridge
  never had. Files go in `dirs::data_dir()/game-koi/<rom-stem>.sav` (data, not config — a save is
  generated state), are written through a temporary plus a rename so a crash cannot truncate one, and are
  autosaved every 120 frames when the RAM is dirty as well as on exit. Only the play path saves; `--test`
  and friends must not, since Blargg's ROMs write their results into SRAM. **MBC3's RTC is not persisted.**
- `src/joypad.rs` — P1. All the active-low inversion lives here; the public API is plain
  `press`/`release`. Selecting a button group means writing its select bit **low**.
- `game-koi-desktop/src/frontend/` — window, input, audio out, frame pacing (winit + pixels + gilrs + cpal + egui). The only module outside the
  emulated machine; the core never depends on it, which is what keeps `--test` and `--screenshot`
  headless. `mod.rs` holds the window, the palette, pacing, and the `Action` enum both input devices
  emit; `keyboard_input.rs` and `game_controller.rs` are pure per-device translation to `Action`, so
  both are unit-testable with no window and no gamepad attached. `stats.rs` holds the host-side
  performance counters (rolling FPS, frame time, late frames) — all wall-clock facts about the
  *emulator*, never about the emulated machine, so nothing here belongs in the core. `overlay.rs` is
  the egui debug panel, hidden by default and toggled with **`** (backtick) — the same key the
  browser build uses, since winit's `KeyCode` and the web's `KeyboardEvent.code` are the same UI
  Events names and so name the same physical key.

  `crt.rs` + `crt.wgsl` are the display post-process, cycled with F3 (`CrtMode`: `Off`, `Scanlines`).
  With an effect on, `ScalingRenderer::render` is pointed at an intermediate texture instead of the
  surface — it takes any texture view, which is what makes this possible without forking `pixels` —
  and the CRT pass reads that back and writes the surface. `Off` skips both, so a feature that is
  switched off costs a branch, not a pass. Three things there are easy to get wrong:

  - **The period comes from the scaler's `clip_rect()`, not the window.** The scaler letterboxes to
    hold 10:9, so dividing the window height by 144 puts the stripes out of step with the lines.
  - **`@builtin(position)` is the pixel *centre*** (`row + 0.5`). Subtract the half-pixel or the
    cosine is sampled half a row out of step; at the usual four rows per line it lands on ±π/4 every
    time, never reaches its extremes, and the profile degenerates into a square wave at about half
    the `strength` asked for. Verified against a screenshot: anchored to the row it measures
    bright/mid/dark/mid, which is the shape a beam had.
  - **The maths happens in linear space, not sRGB**, because the surface format is sRGB and wgpu
    converts on both the sample and the write. That is what you want, and it means the brightness
    compensation clips highlights — measured output matched a linear-space model to within 0.2/255.

  The mask is a cosine rather than an every-other-row test because the period is fractional whenever
  the window is not an exact multiple of 144 tall, and a step function beats against the pixel grid
  into bands that crawl as you resize. Below two rows per line the effect is skipped entirely.

  `stats.rs` also holds `GpuInfo`, the one thing there that is not a measurement: which adapter
  `pixels` settled on, whether it is hardware or a CPU rasteriser (`GpuKind::Software` — llvmpipe,
  lavapipe), the driver, and the backend. Read once in `resumed` because an adapter cannot change
  under a live surface. Only the description is kept, no wgpu handles, so the module still needs no
  GPU to test — and the `DeviceType`/`Backend` matches are exhaustive on purpose, since neither enum
  is `#[non_exhaustive]` and a new variant should break the build rather than be folded into
  "unknown". The software case is unit-tested only; there is no lavapipe here to try it against.

  **A "late" frame is not a dropped one**: every frame the PPU completes is drawn. Late means the
  cycle overran the 16.74 ms budget so the next frame started behind, which is the condition the
  pacing resync in `about_to_wait` already detects. Late also does not imply the machine is too
  slow — with vsync on, the present call blocks until the compositor frees a swapchain image, and a
  59.73 Hz emulator against a 60 Hz display drifts in and out of phase permanently, so frames go
  late while the CPU is barely working. F2 toggles vsync (tearing is the trade).

  **Frame time is split across two winit callbacks** — emulation in `about_to_wait`, drawing in
  `RedrawRequested` — so a single span around either measures a fraction of the frame and reads as
  comfortable no matter how bad things get. `stats.rs` times the phases separately and the total
  deadline-to-deadline. The pacing sleep is tracked as its own phase precisely so it is *not*
  counted as a stall: on a healthy frame it is ~14 ms of the 16.74, and lumping it in with blocking
  makes idle look pathological. `blocked()` is what is left after work and sleep, and is the figure
  worth watching. Measured on an i7-12800H: emulate ~2 ms, render ~0.8 ms — about 18% of budget.

  **egui is pinned to 0.35, and must stay there while `pixels` is on 0.17.** The overlay borrows the
  `wgpu::Device` and `Queue` that `pixels` owns rather than creating its own, so both must agree on
  the wgpu version — `pixels` 0.17.2 uses wgpu 29, and egui-wgpu 0.36 moved to wgpu 30. Mismatching
  them links two semver-incompatible wgpu crates and produces the memorably unhelpful error
  "expected `wgpu::Device`, found `wgpu::Device`". `cargo tree -d` is the check. `wgpu` is not a
  direct dependency: `overlay.rs` uses `egui_wgpu::wgpu`, so there is only one version to get wrong.

  **`App::exiting` is load-bearing, not housekeeping.** It drops the overlay, `pixels` and the
  window while the event loop is still alive. Letting `App`'s own drop do it instead segfaults on
  every exit under Wayland: by the time `run_app` returns, winit has disconnected and freed the
  display, and egui-winit's clipboard is built on a *borrowed* Wayland display pointer rather than
  a refcounted handle — dropping it joins a worker thread that then destroys its protocol objects
  through that dangling pointer. Keep the order the reverse of `resumed`.

  Gamepads are drained once per frame
  in `about_to_wait` because gilrs keeps its own queue outside winit's event stream. `audio.rs` drains the
  APU's queue into cpal through a `Mutex<VecDeque>`; a missing sound device is not an error, it just runs
  silently.
- `src/cartridge/mbc/` — the mapper chips, behind an `Mbc` trait (ROM/RAM read and write, plus a `tick`
  only MBC3 uses). `mod.rs` holds the trait, the factory, and the shared `Rom`/`Ram` chip types that own
  the "bank number plus CPU address becomes a chip offset" wiring — so each of `mbc1.rs`, `mbc2.rs`,
  `mbc3.rs`, `mbc5.rs` is only about its own registers. `NoMbc`, MBC1 (including the MBC1M multicart
  variant), MBC2, MBC3 (+RTC), and MBC5 are implemented; anything else is a typed `UnsupportedMapper`
  error at bus construction rather than a silently wrong run. `Cartridge::create_mbc` is the factory.

  Two things to keep straight, because they differ *between* chips and are easy to unify by mistake: the
  RAM gate decodes the low nibble on MBC1/MBC2/MBC3 but compares all eight bits on MBC5, and only MBC5
  lets bank 0 into the high window. Bank numbers are always masked, never bounds-checked — unconnected
  address lines are why, and Mooneye's `rom_*` tests check for the wrap.
- `src/testbus.rs` — `TestBus`, the stopgap bus wiring the cartridge (through its mapper), WRAM, timer,
  serial, PPU, OAM DMA, and interrupts. Used by `--trace`, `--test`, `--mooneye`, `--screenshot`, and the
  integration tests. I/O addresses no chip implements read **0xFF**, not RAM — see the comment on that
  arm; backing them with RAM makes `cpu_instrs.gb` believe it is on a CGB and hang on `STOP`.

`cargo run -- <rom.gb> --trace [steps]` disassembles and runs the first N instructions against the
stopgap bus. `cargo run --release -- <rom.gb> --test` runs a ROM to completion and prints its serial
output — use release, debug takes minutes. `--mooneye` is the same idea for Mooneye ROMs, which report a
six-byte Fibonacci signature over serial instead of text. `--screenshot [frames]` renders N frames and
writes `screenshot.pgm` (plain-text PGM, so no image crate is needed).

Modes are parsed into one `Mode` enum and an unrecognized flag is an error. Don't reintroduce a
fall-through to windowed play: a typo used to open a window and sit there.

`--frames N` runs N frames, prints the achieved frame rate, and exits — for checking pacing without a
human closing the window. It excludes the first frame, since window and GPU surface creation costs a few
hundred milliseconds and averaging that into a short run makes the rate look much worse than it is.

**Gotcha when writing test ROMs by hand:** the PPU starts with the LCD *on* (LCDC = 0x91, the post-boot
value), so bulk VRAM writes are silently dropped during mode 3. Turn the LCD off first, as real ROMs do —
the symptom is a uniformly blank screen with no error.

Version control is **jj (Jujutsu)** colocated with git (both `.jj/` and `.git/` exist). Use `jj` commands
(`jj st`, `jj log`, `jj diff`) rather than `git` ones. The git repo has no commits yet; history lives in jj.

## Commands

Rust 1.95, edition 2024.

**System libraries.** Two host-side dependencies link against C libraries and need their `-dev` packages
present at build time, not just the runtime library every desktop already has — without them `cargo build`
fails in a build script with "Package … not found in the pkg-config search path", which reads like a Rust
problem but isn't:

- **ALSA** (`alsa-sys`, via `cpal`) — sound output.
- **libudev** (`libudev-sys`, via `gilrs`) — how gilrs enumerates gamepads on Linux.

On Debian/Ubuntu:

```
sudo apt install libasound2-dev libudev-dev pkg-config
```

Elsewhere: `alsa-lib-devel` + `systemd-devel` (Fedora), `alsa-lib` + `systemd-libs` (Arch), `alsa-devel` +
`libudev-devel` (openSUSE). Neither is guarded by a Cargo feature, so both are required even for the
headless modes (`--test`, `--mooneye`, `--screenshot`) and for `cargo test`.

```
cargo run --release -- <rom.gb>   # play a ROM in a window
cargo build --release       # release build (needed for real-time speed; debug is far too slow)
cargo test                  # all tests
cargo test <substring>      # tests whose name contains <substring>
cargo test -- --nocapture   # let println!/debug tracing through
cargo clippy --all-targets  # lint
cargo fmt                   # format

cargo build -p game-koi-core --target wasm32-unknown-unknown   # the core must keep building for the browser
cargo build -p game-koi-web  --target wasm32-unknown-unknown   # the wasm-bindgen wrapper
```

`cargo run`/`cargo test` still work from the repo root and still produce a binary called
`game-koi`, so nothing in the sections above changed shape when the workspace was split.
The browser build needs the target installed once: `rustup target add wasm32-unknown-unknown`.
The browser frontend is built by `crates/game-koi-web/build.sh` (which runs the cargo
build, `wasm-bindgen`, `npm install`, and the npm package's `tsc` build, in that order),
then served statically **from the crate root**, which is where the demo's `index.html`
lives — so the page is at `/` and the `js/dist/` and `js/wasm/` directories it imports
are its siblings. Rooting the server any deeper 404s that import, since a static server
will not serve a path above its own root:

```
./crates/game-koi-web/build.sh
python3 -m http.server -d crates/game-koi-web 8080
# open http://localhost:8080/
```

The `game-koi` npm package itself lives in `crates/game-koi-web/js/`; `npm pack` from
there (after a build) is what would go to the registry. It needs Node installed but
nothing else host-specific — no ALSA/libudev, since it never touches `game-koi-desktop`.

**The ALSA and libudev requirements above are `game-koi-desktop`'s, not the core's.**
`cargo build -p game-koi-core` needs neither, which is what lets the wasm build work on a
machine with no sound stack at all.

## Hardware reference

`external-docs/gbctr.pdf` — gekkio's *GB: Complete Technical Reference* (rev 192). This is the primary
source for hardware behavior; consult it over recalled details when they conflict. Extract text with
`pdftotext -f <first> -l <last> external-docs/gbctr.pdf -`.

Caveat carried from the document itself: it covers **DMG and 2nd-gen devices, pre-Game Boy Color**. Do not
generalize its timing or register behavior to CGB.

**gbctr has no APU chapter** — rev 192 covers the sound registers in the memory map and nothing else, so
Pandocs is the primary source for the APU specifically. `gbdev.io` blocks plain fetches; the Markdown
sources are readable from `raw.githubusercontent.com/gbdev/pandocs/master/src/` (`Audio.md`,
`Audio_Registers.md`, `Audio_details.md`).

Pandocs (gbdev.io/pandocs) is the other standard reference and is easier to skim for memory-map and
register layouts; the PDF is more precise on timing and edge cases.

## Architecture

The shape this kind of emulator takes, and the intended build order:

**Cycle-driven, CPU-stepped.** The CPU executes one instruction, reports how many T-cycles (4 MHz ticks)
it took, and the other components are advanced by that many cycles. Subsystems are not free-running
threads. Getting cycle *accounting* right early matters more than getting it perfectly accurate — the PPU
and timer both derive their behavior from it, and retrofitting timing later is painful.

**The bus is the seam.** Every component talks through a memory bus that decodes addresses into cartridge
ROM/RAM, VRAM, WRAM, OAM, I/O registers, HRAM, and the interrupt-enable byte. Reads and writes are *not*
plain array access: I/O registers have side effects (writing `DIV` resets it to zero, writing `DMA`
launches an OAM transfer), and some regions are inaccessible while the PPU is in certain modes. Keep this
logic in the bus rather than scattering it into callers.

**Ownership tension.** The CPU needs the bus, the bus needs every component, and the PPU needs to raise
interrupts back at the CPU. This is the main place Rust's borrow checker will push back on the obvious
design. The usual resolution: the CPU borrows the bus (`&mut`) only for the duration of a step rather
than owning it, and interrupts are requested by setting bits in the `IF` register on the bus, which the
CPU polls — not by calling into the CPU. Avoid `Rc<RefCell<...>>` webs; they compile but hide the
aliasing bugs that make emulator behavior hard to reason about.

Suggested order, each step ending somewhere runnable:

1. ~~**Cartridge / ROM loading**~~ — done.
2. ~~**CPU + bus skeleton**~~ — done. Decoding is by bit-pattern (`xxyyyzzz`) rather than a 500-arm match;
   see the module docs in `src/cpu/mod.rs` for the scheme. The ALU lives in `src/cpu/alu.rs` as pure
   functions so flag behavior can be tested without a bus.
3. ~~**Timer and interrupts**~~ — done, plus the serial port. All 11 Blargg `cpu_instrs` ROMs and
   `instr_timing` pass.
4. ~~**PPU**~~ — done, plus OAM DMA. Mode 3 uses a fixed 172 T-cycles rather than varying with sprite
   count and scroll, and a scanline is drawn in one go on entering HBlank instead of pixel by pixel — so
   mid-scanline register changes are not modelled. Both are fine for games, not for Mooneye's PPU timing
   tests.
5. ~~**Host frontend**~~ — done. winit 0.30 + pixels 0.17 + gilrs 0.11 + cpal 0.18 + dirs 7 + egui 0.35
   (with egui-wgpu/egui-winit, for the ` stats overlay), the only dependencies, all host-side. Paces on the emulator's own frame completion (59.73 Hz), not the host refresh rate.
   Keyboard and gamepad are both live at once.
6. ~~**MBC1 and beyond**~~ — done. `src/cartridge/mbc/`: MBC1 (+MBC1M), MBC2, MBC3 (+RTC), MBC5, plus
   battery-backed saves in `src/save.rs`. That covers essentially every commercial DMG and CGB cartridge.
   Still missing: **MBC3's RTC is not persisted** across runs, and the exotic mappers (MBC6, MBC7,
   HuC1/3, MMM01, Tama5, Pocket Camera), which together account for a handful of titles.
7. ~~**APU**~~ — done. All four channels, the DIV-APU sequencer, the mixer with its DC-removing high-pass
   filter, and cpal output. Not done: **DMG wave RAM access timing** (see below), and the "zombie mode"
   envelope writes, which are model-dependent even on real hardware.

## Testing

`cargo test` runs unit tests; `cargo test --release` also runs the ROM suites (release, or they crawl).
`tests/blargg.rs` and `tests/mooneye.rs` shell out to the built binary's `--test` and `--mooneye` modes.

**Currently passing:** all 11 Blargg `cpu_instrs` ROMs, the combined `cpu_instrs.gb`, `instr_timing`,
9 of the 12 Blargg `dmg_sound` ROMs, and all 28 Mooneye `emulator-only` mapper ROMs (13 mbc1, 7 mbc2,
8 mbc5).

**Two Blargg reporting conventions.** Most suites print to the serial port; `dmg_sound` instead writes a
status byte and its text into cartridge RAM at 0xA000 behind the signature `DE B0 61`. `--test` checks
both, because a ROM using the second one produces no serial output and then spins in `jr $` — which looks
exactly like a hang if you only know the first.

**Frame length is correct, and a mean over frames will tell you otherwise.** Measured on
Tetris in steady state, 999 of 1000 frames take **17554-17558 M-cycles against the ideal
17556** (154 lines x 114), which is 59.73 Hz to within 0.005%. The APU tracks it: 803.6
samples per frame where 48000/59.7275 = 803.65.

The trap is that **a frame during which the game turns the LCD off is arbitrarily long**.
`Ppu::tick` returns immediately when LCDC bit 7 is clear, so `frame_ready` never fires
and the `while !frame_ready` loop keeps stepping until the game switches the LCD back on
— Tetris's boot does this for two frames of 106160 and 75937 M-cycles, and something
later does it again for 84038. That is correct hardware behaviour, not a stall: a real
DMG with its LCD off is not producing frames either. But three outliers in a thousand are
enough to drag a naive mean from 17555 to 17733 and invent a 1-2% timing error that is
not there. Exclude LCD-off frames before averaging anything.

**The APU's known gap** is DMG wave RAM access timing: on hardware the CPU can reach wave RAM on the exact
cycle CH3 reads a sample, and gets the byte CH3 is reading whatever address it asked for. This bus ticks
whole M-cycles with CPU reads outside them, so it cannot express "the same cycle". Blargg's sound tests
`09`, `10` and `12` are the only things that notice.

**MBC3 has no ROM coverage** — Mooneye ships no `mbc3` group — so its banking and its RTC rest on unit
tests alone. Treat it as the least-proven mapper.

The ROMs live in `test-roms/` and are **not committed** (gitignored):

- Blargg, from <https://github.com/retrio/gb-test-roms> — `test-roms/cpu_instrs.gb`,
  `test-roms/instr_timing.gb`, `test-roms/individual/`, `test-roms/dmg_sound/` (from `rom_singles/`). The individual filenames contain spaces and
  commas, so raw URLs need percent-encoding.
- Mooneye, from <https://gekkio.fi/files/mooneye-test-suite/> (built ROMs; the GitHub repo has no
  releases) — put the `emulator-only/mbc1`, `mbc2` and `mbc5` directories under `test-roms/mooneye/`.

When the ROMs are absent the tests **skip rather than fail**, so a fresh clone stays green — which also
means a passing run proves nothing if the directory is empty. Check for skip lines with
`cargo test --release -- --nocapture` before trusting a green run.

The combined `cpu_instrs.gb` (64 KiB, MBC1) is the banking regression test: the 11 individual ROMs are
all 32 KiB and exercise no mapper at all. Two things had to be right before it passed, and only one of
them was banking — the other was unimplemented I/O reading 0xFF. Mooneye's *timing* suite is the next
rung up from here, and it will need the PPU's mode-3 timing fixed before it can pass.

Watch for: `cpu_instrs.gb` ships with a **deliberately wrong global checksum**, which is why the header
parser records checksums rather than enforcing them.

Unit-test the fiddly, self-contained pieces directly: flag computation for `ADC`/`SBC`/`DAA`, the
half-carry cases, MBC bank-number masking, and memory-map decode boundaries.
