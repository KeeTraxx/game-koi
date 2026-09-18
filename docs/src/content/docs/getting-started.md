---
title: Getting started
description: Install the game-koi npm package and get it running on a page.
---

`game-koi` is published to npm as the browser build of the emulator core — a
WebAssembly module wrapped in a small TypeScript API that owns the canvas rendering,
audio, and pacing loop for you.

```sh
npm install game-koi
```

```ts
import { GameKoi } from "game-koi";

const canvas = document.querySelector("canvas")!;
const romBytes = new Uint8Array(await (await fetch("/game.gb")).arrayBuffer());

// Call from inside a user gesture (e.g. a click handler) — browsers suspend a
// freshly created AudioContext otherwise.
button.addEventListener("click", async () => {
  const koi = await GameKoi.create({ canvas, rom: romBytes });
});
```

See it running against a ROM you supply yourself in [Examples: ROM
player](/examples/rom-player/).

## Full API

This site only shows enough to get a page running. For the complete API — gamepad
mapping, the debug overlay, pausing/disposing, and every `GameKoi.create` option — see
the package's own README:
[`crates/game-koi-web/js/README.md`](https://github.com/KeeTraxx/game-koi/blob/main/crates/game-koi-web/js/README.md)
in the source repository.
