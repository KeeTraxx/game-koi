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

Steps 1 and 2 of the build order below are done; steps 3-7 are not started. `[dependencies]` is still
empty and nothing draws to a screen. Treat the unfinished steps as a plan, and verify against the actual
tree before assuming a subsystem exists.

- `src/cartridge/` — ROM loading and header parsing. No mapper support yet: only 32 KiB no-MBC ROMs.
- `src/bus.rs` — the `Bus` trait (`read`/`write`/`tick`). The real hardware bus does not exist yet; the
  only implementations are `FlatMemory` for tests and a stopgap in `main.rs` for tracing.
- `src/cpu/` — the SM83 core. All 256 opcodes plus the 256-entry `CB` page decode and execute, with
  per-instruction cycle counts. Interrupt *dispatch* is not implemented: `IME`, `DI`, `EI`, and `RETI`
  maintain the flag, but nothing checks `IE`/`IF` or jumps to a handler yet — that arrives with step 3,
  and until then a halted CPU never wakes.

`cargo run -- <rom.gb> --trace [steps]` disassembles and runs the first N instructions of a ROM against
the stopgap bus. Useful for eyeballing CPU behavior against a known disassembly.

Version control is **jj (Jujutsu)** colocated with git (both `.jj/` and `.git/` exist). Use `jj` commands
(`jj st`, `jj log`, `jj diff`) rather than `git` ones. The git repo has no commits yet; history lives in jj.

## Commands

Rust 1.95, edition 2024.

```
cargo run -- <rom.gb>       # run the emulator against a ROM
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
3. **Timer and interrupts** — `DIV`/`TIMA`/`TMA`/`TAC`, the `IE`/`IF` pair, and the interrupt dispatch
   sequence. This is what makes test ROMs able to report results.
4. **PPU** — the mode 2/3/0/1 state machine per scanline, tile/background rendering, then sprites and
   window. Produce a framebuffer; keep it separate from how it's displayed.
5. **Host frontend** — put the framebuffer on screen and wire up joypad input. Deliberately last, and
   kept behind a boundary, so the emulator core stays testable without a window.
6. **MBC1 and beyond** — bank switching, once no-MBC games work.
7. **APU** — audio is genuinely the hardest to get right and the least necessary; leave it for the end.

## Testing

The standard approach for this domain, worth adopting early: run the community test ROMs (Blargg's
`cpu_instrs` and `instr_timing`, later Mooneye's timing suite) as automated tests. They execute on the
emulator itself and report pass/fail — via the serial port, which is why a stub serial output that
collects written bytes into a string is worth having as soon as the CPU runs.

Blargg's `cpu_instrs` split into its 11 individual ROMs makes a good incremental target: each one passing
is a real checkpoint. Test ROM binaries should not be committed — fetch them into an ignored directory.

Unit-test the fiddly, self-contained pieces directly: flag computation for `ADC`/`SBC`/`DAA`, the
half-carry cases, MBC bank-number masking, and memory-map decode boundaries.
