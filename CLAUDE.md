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

Steps 1-5 of the build order below are done; steps 6-7 are not started. The emulator opens a window and
is playable, but only for 32 KiB no-MBC ROMs — most commercial games need step 6. Treat the unfinished
steps as a plan, and verify against the actual tree before assuming a subsystem exists.

- `src/cartridge/` — ROM loading and header parsing. No mapper support yet: only 32 KiB no-MBC ROMs.
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
- `src/joypad.rs` — P1. All the active-low inversion lives here; the public API is plain
  `press`/`release`. Selecting a button group means writing its select bit **low**.
- `src/frontend/` — window, input, frame pacing (winit + pixels + gilrs). The only module outside the
  emulated machine; the core never depends on it, which is what keeps `--test` and `--screenshot`
  headless. `mod.rs` holds the window, the palette, pacing, and the `Action` enum both input devices
  emit; `keyboard_input.rs` and `game_controller.rs` are pure per-device translation to `Action`, so
  both are unit-testable with no window and no gamepad attached. Gamepads are drained once per frame
  in `about_to_wait` because gilrs keeps its own queue outside winit's event stream.
- `src/cartridge/mbc.rs` — the mapper chips, behind an `Mbc` trait of four operations (ROM/RAM read and
  write). `NoMbc` and `Mbc1` are implemented; anything else is a typed `UnsupportedMapper` error at bus
  construction rather than a silently wrong run. `Cartridge::create_mbc` is the factory.
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

```
cargo run --release -- <rom.gb>   # play a ROM in a window
cargo build --release       # release build (needed for real-time speed; debug is far too slow)
cargo test                  # all tests
cargo test <substring>      # tests whose name contains <substring>
cargo test -- --nocapture   # let println!/debug tracing through
cargo clippy --all-targets  # lint
cargo fmt                   # format
```

## Hardware reference

`external-docs/gbctr.pdf` — gekkio's *GB: Complete Technical Reference* (rev 192). This is the primary
source for hardware behavior; consult it over recalled details when they conflict. Extract text with
`pdftotext -f <first> -l <last> external-docs/gbctr.pdf -`.

Caveat carried from the document itself: it covers **DMG and 2nd-gen devices, pre-Game Boy Color**. Do not
generalize its timing or register behavior to CGB.

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
5. ~~**Host frontend**~~ — done. winit 0.30 + pixels 0.17 + gilrs 0.11, the only dependencies, all
   host-side. Paces on the emulator's own frame completion (59.73 Hz), not the host refresh rate.
   Keyboard and gamepad are both live at once.
6. ~~**MBC1**~~ — done. `src/cartridge/mbc.rs`. Not done: **MBC1M**, the multicart variant with a 4-bit
   BANK1, which cannot be identified from the header (emulators sniff for repeated Nintendo logos), and
   **battery-backed saves** — cartridge RAM is emulated but never written to disk, so saves die with the
   process. `Cartridge::path` is already kept for naming a save file. MBC3 (+RTC) and MBC5 are next.
7. **APU** — audio is genuinely the hardest to get right and the least necessary; leave it for the end.

## Testing

`cargo test` runs unit tests; `cargo test --release` also runs the ROM suites (release, or they crawl).
`tests/blargg.rs` and `tests/mooneye.rs` shell out to the built binary's `--test` and `--mooneye` modes.

**Currently passing:** all 11 Blargg `cpu_instrs` ROMs, the combined `cpu_instrs.gb`, `instr_timing`, and
12 of the 13 Mooneye `emulator-only/mbc1` ROMs. The exception is `multicart_rom_8Mb` (MBC1M), which is
not implemented and is deliberately not listed as a test.

The ROMs live in `test-roms/` and are **not committed** (gitignored):

- Blargg, from <https://github.com/retrio/gb-test-roms> — `test-roms/cpu_instrs.gb`,
  `test-roms/instr_timing.gb`, `test-roms/individual/`. The individual filenames contain spaces and
  commas, so raw URLs need percent-encoding.
- Mooneye, from <https://gekkio.fi/files/mooneye-test-suite/> (built ROMs; the GitHub repo has no
  releases) — put `emulator-only/mbc1` at `test-roms/mooneye/mbc1/`.

When the ROMs are absent the tests **skip rather than fail**, so a fresh clone stays green — which also
means a passing run proves nothing if the directory is empty. Check for skip lines with
`cargo test --release -- --nocapture` before trusting a green run.

The combined `cpu_instrs.gb` (64 KiB, MBC1) is the banking regression test: the 11 individual ROMs are
all 32 KiB and exercise no mapper at all. Two things had to be right before it passed, and only one of
them was banking — the other was unimplemented I/O reading 0xFF. Mooneye's *timing* suite is the next
rung up from here.

Watch for: `cpu_instrs.gb` ships with a **deliberately wrong global checksum**, which is why the header
parser records checksums rather than enforcing them.

Unit-test the fiddly, self-contained pieces directly: flag computation for `ADC`/`SBC`/`DAA`, the
half-carry cases, MBC bank-number masking, and memory-map decode boundaries.
