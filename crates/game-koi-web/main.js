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
const buttonsEl = document.getElementById("buttons");

let koi = null;

// One pill per button, lit from the emulator's press/release events. Whichever device
// produced the edge — keyboard, gamepad, or a press() call — arrives the same way here.
const BUTTONS = ["up", "down", "left", "right", "a", "b", "start", "select"];
const pills = new Map(
  BUTTONS.map((button) => {
    const el = document.createElement("span");
    el.textContent = button;
    buttonsEl.append(el);
    return [button, el];
  }),
);

/** Redraws from the event's snapshot rather than toggling one pill, so the display
    cannot drift out of step with the joypad however the edges arrive. */
function showPressed(pressed) {
  for (const [button, el] of pills) el.classList.toggle("held", pressed.has(button));
}

fileInput.addEventListener("change", async (event) => {
  const file = event.target.files[0];
  if (!file) return;

  try {
    const rom = new Uint8Array(await file.arrayBuffer());
    if (koi) {
      koi.loadRom(rom);
    } else {
      koi = await GameKoi.create({ canvas, rom });
      koi.on("press", (event) => showPressed(event.pressed));
      koi.on("release", (event) => showPressed(event.pressed));
    }
    statusEl.textContent = `running ${file.name}`;
  } catch (err) {
    // A bad header or an unsupported mapper arrives here as a thrown JsError.
    statusEl.textContent = `error: ${err}`;
    koi = null;
  }
});

statusEl.textContent = "ready — choose a ROM";
