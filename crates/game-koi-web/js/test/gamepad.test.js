// Tests for the gamepad mapping, run with node's built-in test runner:
//
//   npm test        (builds first — these import the compiled dist/)
//
// Plain JavaScript against the build output rather than TypeScript against src/, so
// there is no test-only toolchain to install and nothing test-shaped ends up in the
// published package. The module under test is pure — it takes pad snapshots and
// returns edges — so none of this needs a browser or a real controller.

import assert from "node:assert/strict";
import { test } from "node:test";

import { DEFAULT_GAMEPAD_MAP, GamepadInput, diff, heldButtons } from "../dist/gamepad.js";

/** A standard-mapping pad with the given button indices down and the stick centred. */
function pad(pressed = [], axes = [0, 0]) {
  const buttons = Array.from({ length: 17 }, (_, i) => ({ pressed: pressed.includes(i) }));
  return { mapping: "standard", buttons, axes };
}

test("the default map covers all eight Game Boy buttons", () => {
  const mapped = Object.values(DEFAULT_GAMEPAD_MAP);
  assert.equal(new Set(mapped).size, 8);
  // Position, not printed label: index 1 is the right-hand face button, which is where
  // A sits on the DMG's diagonal.
  assert.equal(DEFAULT_GAMEPAD_MAP[1], "a");
  assert.equal(DEFAULT_GAMEPAD_MAP[0], "b");
  // Buttons the Game Boy does not have stay unmapped.
  assert.equal(DEFAULT_GAMEPAD_MAP[6], undefined);
  assert.equal(DEFAULT_GAMEPAD_MAP[16], undefined);
});

test("face buttons and the d-pad are held", () => {
  const held = heldButtons([pad([1, 9, 14])], DEFAULT_GAMEPAD_MAP);
  assert.deepEqual([...held].sort(), ["a", "left", "start"]);
});

test("a pad the browser could not identify is ignored", () => {
  // Indices in a non-standard layout are whatever the driver enumerated, so binding
  // them would be confidently wrong rather than merely absent.
  const unknown = { ...pad([1]), mapping: "" };
  assert.equal(heldButtons([unknown], DEFAULT_GAMEPAD_MAP).size, 0);
});

test("a disconnected slot is skipped", () => {
  // getGamepads() returns a sparse array with nulls where a pad has gone away.
  const held = heldButtons([null, pad([12])], DEFAULT_GAMEPAD_MAP);
  assert.deepEqual([...held], ["up"]);
});

test("a stick at rest holds nothing", () => {
  assert.equal(heldButtons([pad([], [0, 0])], DEFAULT_GAMEPAD_MAP).size, 0);
  // Drift below the threshold must not hold a direction.
  assert.equal(heldButtons([pad([], [0.3, -0.3])], DEFAULT_GAMEPAD_MAP).size, 0);
});

test("a pushed stick acts as a d-pad, with +Y meaning down", () => {
  assert.deepEqual([...heldButtons([pad([], [-1, 0])], DEFAULT_GAMEPAD_MAP)], ["left"]);
  assert.deepEqual([...heldButtons([pad([], [0, 1])], DEFAULT_GAMEPAD_MAP)], ["down"]);
  // A diagonal has to register on both axes, which is what the 0.5 threshold buys.
  const diagonal = heldButtons([pad([], [0.7, -0.7])], DEFAULT_GAMEPAD_MAP);
  assert.deepEqual([...diagonal].sort(), ["right", "up"]);
});

test("opposing directions from two sources cancel", () => {
  // A d-pad is one rocker; Left+Right is a state the hardware cannot produce and some
  // games handle badly. Neutral is the closest reachable answer.
  const held = heldButtons([pad([14], [1, 0])], DEFAULT_GAMEPAD_MAP);
  assert.equal(held.size, 0);
});

test("every connected pad drives the one player", () => {
  const held = heldButtons([pad([0]), pad([9])], DEFAULT_GAMEPAD_MAP);
  assert.deepEqual([...held].sort(), ["b", "start"]);
});

test("a release is emitted before a press", () => {
  // Flicking a stick from Left through centre to Right must not leave Left down while
  // Right is applied.
  const actions = diff(new Set(["left"]), new Set(["right"]));
  assert.deepEqual(actions, [
    { type: "release", button: "left" },
    { type: "press", button: "right" },
  ]);
});

test("holding a button presses it once", () => {
  const input = new GamepadInput(DEFAULT_GAMEPAD_MAP);

  assert.deepEqual(input.poll([pad([1])]), [{ type: "press", button: "a" }]);
  // Still down, nothing changed: no repeat press.
  assert.deepEqual(input.poll([pad([1])]), []);
  assert.deepEqual(input.poll([pad([])]), [{ type: "release", button: "a" }]);
});

test("a pad vanishing releases what it held", () => {
  const input = new GamepadInput(DEFAULT_GAMEPAD_MAP);
  input.poll([pad([12])]);
  assert.deepEqual(input.poll([]), [{ type: "release", button: "up" }]);
});

test("releaseAll lets go of everything and forgets it", () => {
  const input = new GamepadInput(DEFAULT_GAMEPAD_MAP);
  input.poll([pad([1, 12])]);

  const released = input.releaseAll().map((action) => action.button).sort();
  assert.deepEqual(released, ["a", "up"]);
  // Forgotten, so the same pad state reads as a fresh press rather than as unchanged.
  assert.deepEqual(input.poll([pad([1])]), [{ type: "press", button: "a" }]);
});

test("a custom map replaces the button bindings but not the stick", () => {
  const swapped = { 0: "a", 1: "b" };
  assert.deepEqual([...heldButtons([pad([0])], swapped)], ["a"]);
  // Directions are not indices, so the stick keeps working with a map that has none.
  assert.deepEqual([...heldButtons([pad([], [0, -1])], swapped)], ["up"]);
});
