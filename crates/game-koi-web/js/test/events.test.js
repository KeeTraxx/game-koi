// Tests for the typed emitter behind `koi.on`/`koi.off`, run with node's built-in test
// runner:
//
//   npm test        (builds first — these import the compiled dist/)
//
// Same shape as gamepad.test.js: plain JavaScript against the build output, no browser.
// `events.ts`'s only import of index.ts is `import type`, so it erases at compile time
// and this module can be loaded without dragging in the wasm glue.

import assert from "node:assert/strict";
import { test } from "node:test";

import { Emitter } from "../dist/events.js";

test("listeners receive the payload for their own event only", () => {
  const emitter = new Emitter();
  const presses = [];
  const releases = [];
  emitter.on("press", (event) => presses.push(event.button));
  emitter.on("release", (event) => releases.push(event.button));

  emitter.emit("press", { button: "a", source: "keyboard", pressed: new Set(["a"]) });
  emitter.emit("release", { button: "a", source: "keyboard", pressed: new Set() });

  assert.deepEqual(presses, ["a"]);
  assert.deepEqual(releases, ["a"]);
});

test("every listener for an event is called, in registration order", () => {
  const emitter = new Emitter();
  const order = [];
  emitter.on("press", () => order.push("first"));
  emitter.on("press", () => order.push("second"));

  emitter.emit("press", { button: "start", source: "api", pressed: new Set(["start"]) });

  assert.deepEqual(order, ["first", "second"]);
});

test("the returned unsubscribe removes the listener, and is idempotent", () => {
  const emitter = new Emitter();
  let calls = 0;
  const stop = emitter.on("press", () => calls++);

  stop();
  stop();
  emitter.emit("press", { button: "b", source: "api", pressed: new Set(["b"]) });

  assert.equal(calls, 0);
});

test("off removes only the listener it was given", () => {
  const emitter = new Emitter();
  let kept = 0;
  const dropped = () => assert.fail("removed listener was called");
  emitter.on("press", () => kept++);
  emitter.on("press", dropped);

  emitter.off("press", dropped);
  emitter.emit("press", { button: "up", source: "gamepad", pressed: new Set(["up"]) });

  assert.equal(kept, 1);
});

test("a listener that throws does not stop the ones after it", () => {
  // These fire from inside a keydown handler and from the render loop, so an exception
  // escaping here would skip the rest of a gamepad poll or kill the loop outright. A
  // page's display bug must not stop the emulator.
  const emitter = new Emitter();
  let reached = false;
  const errors = [];
  const consoleError = console.error;
  console.error = (...args) => errors.push(args);

  try {
    emitter.on("press", () => {
      throw new Error("boom");
    });
    emitter.on("press", () => {
      reached = true;
    });

    emitter.emit("press", { button: "a", source: "api", pressed: new Set(["a"]) });
  } finally {
    console.error = consoleError;
  }

  assert.equal(reached, true);
  assert.equal(errors.length, 1);
});

test("unsubscribing during a dispatch does not disturb it", () => {
  // The walk is over a copy, so a listener that removes itself (or a later one) mid
  // dispatch cannot make the iterator skip an entry.
  const emitter = new Emitter();
  const called = [];
  const stopSelf = emitter.on("press", () => {
    called.push("first");
    stopSelf();
    stopNext();
  });
  const stopNext = emitter.on("press", () => called.push("second"));
  emitter.on("press", () => called.push("third"));

  emitter.emit("press", { button: "a", source: "api", pressed: new Set(["a"]) });

  assert.deepEqual(called, ["first", "second", "third"]);
  // Both removals took effect for the next dispatch.
  called.length = 0;
  emitter.emit("press", { button: "a", source: "api", pressed: new Set(["a"]) });
  assert.deepEqual(called, ["third"]);
});

test("emitting an event nobody listens for is a no-op", () => {
  const emitter = new Emitter();
  emitter.emit("release", { button: "a", source: "api", pressed: new Set() });
});

test("clear drops every listener", () => {
  const emitter = new Emitter();
  emitter.on("press", () => assert.fail("listener survived clear()"));
  emitter.on("release", () => assert.fail("listener survived clear()"));

  emitter.clear();
  emitter.emit("press", { button: "a", source: "api", pressed: new Set(["a"]) });
  emitter.emit("release", { button: "a", source: "api", pressed: new Set() });
});
