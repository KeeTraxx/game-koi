// The browser frontend's demo page — deliberately thin. It exists to exercise the
// npm package the same way any consumer would, so this file is also the package's
// integration test: if GameKoi stops being a drop-in, this page is where that shows.
//
// This sits at the crate root rather than in a subdirectory so that serving the crate
// puts the demo at `/`. The package it imports has to be reachable from the same
// document root — a server rooted at a subdirectory cannot serve js/dist above itself —
// and being siblings is what makes that true without copying the built package around.

import { GameKoi } from "./js/dist/index.js";

const canvas = document.getElementById("screen");
const statusEl = document.getElementById("status");
const fileInput = document.getElementById("rom");

let koi = null;

fileInput.addEventListener("change", async (event) => {
  const file = event.target.files[0];
  if (!file) return;

  try {
    const rom = new Uint8Array(await file.arrayBuffer());
    if (koi) {
      koi.loadRom(rom);
    } else {
      koi = await GameKoi.create({ canvas, rom });
    }
    statusEl.textContent = `running ${file.name}`;
  } catch (err) {
    // A bad header or an unsupported mapper arrives here as a thrown JsError.
    statusEl.textContent = `error: ${err}`;
    koi = null;
  }
});

statusEl.textContent = "ready — choose a ROM";
