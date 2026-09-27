# game-koi

[![CI](https://github.com/KeeTraxx/game-koi/actions/workflows/ci.yml/badge.svg)](https://github.com/KeeTraxx/game-koi/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/game-koi)](https://www.npmjs.com/package/game-koi)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A Game Boy (DMG) emulator written in Rust. It runs as a native desktop app and in the
browser via WebAssembly. The browser build is published to npm as `game-koi`.

**Documentation and live demo:** <https://keetraxx.github.io/game-koi/> (also on
[readthedocs](https://game-koi.readthedocs.io/))

## Features

- **CPU:** a complete SM83 core, with all 512 opcodes (including the `CB` page),
  per-instruction cycle counts, interrupts, and the HALT bug.
- **Graphics:** background, window and sprites, with VRAM/OAM access locking and OAM DMA.
- **Sound:** all four APU channels, the frame sequencer, and a stereo mixer.
- **Cartridges:** no mapper, MBC1 (including MBC1M multicarts), MBC2, MBC3 with the
  real-time clock, and MBC5. Together these cover almost every commercial DMG cartridge.
- **Saves:** battery-backed cartridge RAM is saved to disk automatically.
- **Input:** keyboard and gamepad, both active at the same time.
- **Desktop extras:** an optional CRT scanline effect and a performance overlay.
- **Browser:** a drop-in npm package that handles the canvas, audio, frame pacing and
  input for you.

### Test ROM status

- Blargg's `cpu_instrs` (all 11 ROMs plus the combined ROM) and `instr_timing` pass.
- 9 of the 12 Blargg `dmg_sound` ROMs pass.
- All 28 Mooneye `emulator-only` mapper tests pass.

## Project layout

| Path                        | What it is                                                                   |
| --------------------------- | ---------------------------------------------------------------------------- |
| `crates/game-koi-core/`     | The emulated machine. Platform-independent, and builds for `wasm32-unknown-unknown`. |
| `crates/game-koi-desktop/`  | The `game-koi` desktop binary (winit, pixels, cpal, gilrs, egui).            |
| `crates/game-koi-web/`      | The wasm-bindgen bindings, the `game-koi` npm package (`js/`) and a demo page. |
| `docs/`                     | The documentation site (Astro + Starlight).                                  |

## Desktop

### Requirements

- Rust. The toolchain is pinned in `rust-toolchain.toml`, and rustup installs it
  automatically.
- ALSA and libudev development headers (for sound and gamepads):

  ```sh
  # Debian / Ubuntu
  sudo apt install libasound2-dev libudev-dev pkg-config
  # Fedora
  sudo dnf install alsa-lib-devel systemd-devel
  # Arch
  sudo pacman -S alsa-lib systemd-libs
  ```

### Running

```sh
cargo run --release -- path/to/game.gb
```

Always use a release build. A debug build is far too slow to run at full speed.

Battery-backed saves are stored in your platform data directory, for example
`~/.local/share/game-koi/<rom-name>.sav` on Linux.

### Controls

| Game Boy    | Keyboard    | Gamepad          |
| ----------- | ----------- | ---------------- |
| D-pad       | Arrow keys  | D-pad / left stick |
| A           | `Z`         | East (B on Xbox) |
| B           | `X`         | South (A on Xbox) |
| Start       | `Enter`     | Start            |
| Select      | Right Shift | Select           |

| Key  | Action                         |
| ---- | ------------------------------ |
| `P`  | Pause / resume                 |
| `` ` `` | Toggle the performance overlay |
| `F2` | Toggle vsync                   |
| `F3` | Cycle the CRT effect           |
| `Esc` | Quit                          |

### Other modes

```sh
game-koi <rom> --info              # print the cartridge header
game-koi <rom> --trace [steps]     # disassemble and trace the first N instructions
game-koi <rom> --test              # run a Blargg test ROM and print its result
game-koi <rom> --mooneye           # run a Mooneye test ROM and report pass/fail
game-koi <rom> --screenshot [n]    # render n frames headlessly to screenshot.pgm
game-koi <rom> --frames n          # play n frames, then print the achieved frame rate
```

## Browser / npm

```sh
npm install game-koi
```

```ts
import { GameKoi } from "game-koi";

const canvas = document.querySelector("canvas")!;
const rom = new Uint8Array(await (await fetch("/game.gb")).arrayBuffer());

// Create from inside a user gesture, since browsers suspend audio otherwise.
button.addEventListener("click", async () => {
  const koi = await GameKoi.create({ canvas, rom });
});
```

See the [package README](crates/game-koi-web/js/README.md) for the full API and the
[documentation site](https://keetraxx.github.io/game-koi/) for running examples.

To build the package and run the demo page locally, you need Node and
`wasm-bindgen-cli` 0.2.128. The CLI version must match the crate exactly.

```sh
cargo install wasm-bindgen-cli --version 0.2.128
./crates/game-koi-web/build.sh
python3 -m http.server -d crates/game-koi-web 8080   # open http://localhost:8080/
```

## Development

Common tasks are in the [`justfile`](justfile). Run `just` to list them all.

```sh
just test          # unit tests
just test-roms     # unit tests plus the Blargg/Mooneye ROM suites (release)
just lint          # clippy
just fmt           # rustfmt
just check-wasm    # make sure the core still builds for the browser
just test-web      # build and test the npm package
just serve-docs    # build and preview the documentation site
```

### Test ROMs

The ROM suites need test ROMs, which are not included in the repository. Put them in
`test-roms/`:

- **Blargg**, from [retrio/gb-test-roms](https://github.com/retrio/gb-test-roms):
  `cpu_instrs.gb`, `instr_timing.gb`, `individual/`, and `dmg_sound/` (taken from
  that suite's `rom_singles/`).
- **Mooneye**, from the [built test suite](https://gekkio.fi/files/mooneye-test-suite/):
  the `emulator-only/mbc1`, `mbc2` and `mbc5` directories, placed under
  `test-roms/mooneye/`.

If the ROMs are missing, those tests are skipped, not failed. Use
`cargo test --release -- --nocapture` to see which ones ran.

### Releasing

See [RELEASE.md](RELEASE.md).

## Known limitations

- Game Boy Color (CGB) mode is not supported. The emulator targets the original DMG.
- PPU mode 3 has a fixed length, and each scanline is drawn in a single pass. Games run
  correctly, but register changes made partway through a scanline have no effect.
- Wave RAM access timing is not exact. This is why Blargg's `dmg_sound` tests 09, 10 and
  12 fail.
- The MBC3 real-time clock is not saved between sessions.
- Rare mappers (MBC6, MBC7, HuC1/HuC3, MMM01, TAMA5, Pocket Camera) are not supported.

## References

- Gekkio, [*Game Boy: Complete Technical Reference*](https://gekkio.fi/files/gb-docs/gbctr.pdf)
- [Pan Docs](https://gbdev.io/pandocs/)

## License

[MIT](LICENSE) © 2026 Khôi Tran

Game Boy is a trademark of Nintendo. This project is not affiliated with or endorsed by
Nintendo. No ROMs are included. Only use ROMs you have the right to use.
