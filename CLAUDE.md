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
- `src/save.rs` — battery-backed saves. Only for cartridges whose header says they have a battery: RAM on
  a battery-less board is volatile on hardware too, so persisting it would invent a memory the cartridge
  never had. Files go in `dirs::data_dir()/gbemu-rs/<rom-stem>.sav` (data, not config — a save is
  generated state), are written through a temporary plus a rename so a crash cannot truncate one, and are
  autosaved every 120 frames when the RAM is dirty as well as on exit. Only the play path saves; `--test`
  and friends must not, since Blargg's ROMs write their results into SRAM. **MBC3's RTC is not persisted.**
- `src/joypad.rs` — P1. All the active-low inversion lives here; the public API is plain
  `press`/`release`. Selecting a button group means writing its select bit **low**.
- `src/frontend/` — window, input, audio out, frame pacing (winit + pixels + gilrs + cpal). The only module outside the
  emulated machine; the core never depends on it, which is what keeps `--test` and `--screenshot`
  headless. `mod.rs` holds the window, the palette, pacing, and the `Action` enum both input devices
  emit; `keyboard_input.rs` and `game_controller.rs` are pure per-device translation to `Action`, so
  both are unit-testable with no window and no gamepad attached. Gamepads are drained once per frame
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
5. ~~**Host frontend**~~ — done. winit 0.30 + pixels 0.17 + gilrs 0.11 + cpal 0.18 + dirs 7, the only
   dependencies, all host-side. Paces on the emulator's own frame completion (59.73 Hz), not the host refresh rate.
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
