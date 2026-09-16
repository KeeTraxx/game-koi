/**
 * Gamepad input, via the browser's Gamepad API.
 *
 * Unlike the keyboard, a gamepad is not a source of events here: the Gamepad API has
 * `gamepadconnected`/`gamepaddisconnected` and nothing else — a button press fires no
 * event at all. `navigator.getGamepads()` hands back a *snapshot* of every pad's
 * current state, so this is polled once per wake of the render loop and diffed against
 * the previous snapshot to recover the press/release edges the joypad wants. One frame
 * (~16.7 ms) of latency is the granularity the emulator already runs at, so polling
 * more often would buy nothing.
 *
 * Polling is also what makes a pad appear at all: Chrome exposes no pads until one has
 * been interacted with, and reveals them through a later `getGamepads()` call rather
 * than retroactively. Nothing extra is needed to pick that up.
 *
 * # Analog sticks are not a d-pad
 *
 * The DMG's d-pad is four switches: a direction is held or it isn't. A stick reports a
 * continuous position, so it is thresholded into at most one direction per axis. The
 * diff against the previous snapshot is what keeps a resting-at-0.7 stick from
 * re-pressing Right on every frame.
 */

import type { Button } from "./index.js";

/**
 * The part of the browser's `Gamepad` this module reads.
 *
 * Narrowed to the two fields that matter (a real `Gamepad` is structurally compatible)
 * so the mapping can be unit-tested with plain objects and no browser attached.
 */
export interface PadState {
  /** `"standard"` iff the browser recognised the layout; see `heldButtons`. */
  mapping: string;
  buttons: readonly { readonly pressed: boolean }[];
  axes: readonly number[];
}

/** A press or release edge, to be applied to the emulator's joypad. */
export interface Action {
  type: "press" | "release";
  button: Button;
}

/**
 * Standard-layout button indices mapped to Game Boy buttons.
 *
 * A and B sit on a diagonal on the DMG — B lower-left, A upper-right — so the East and
 * South face buttons reproduce that under the thumb. The Gamepad API reports face
 * buttons by *position*, not by printed label: index 0 is always the bottom button and
 * index 1 the right-hand one, whatever a given pad prints on them.
 */
export const DEFAULT_GAMEPAD_MAP: Readonly<Record<number, Button>> = {
  0: "b", // South
  1: "a", // East
  8: "select", // Back/Select
  9: "start",
  12: "up",
  13: "down",
  14: "left",
  15: "right",
};

/**
 * How far a stick must be pushed before it counts as a direction press.
 *
 * Half deflection, the same figure the desktop build uses: high enough that a drifting
 * or off-centre stick does not hold a direction on its own, low enough that a diagonal
 * registers on both axes.
 */
const STICK_THRESHOLD = 0.5;

/** Standard-layout axis indices. The right stick (2, 3) is deliberately ignored. */
const LEFT_STICK_X = 0;
const LEFT_STICK_Y = 1;

/**
 * Everything held across every connected pad, as a set of Game Boy buttons.
 *
 * Every pad is accepted rather than picking a "player 1": the Game Boy has one player,
 * so asking which controller owns it would be ceremony for nothing.
 *
 * Pads whose `mapping` is not `"standard"` are skipped entirely. The indices in a
 * non-standard layout are whatever the driver happened to enumerate, so guessing at
 * them produces confidently wrong bindings rather than none; a page with such a pad can
 * pass `gamepadMap: null` and drive `press`/`release` itself.
 */
export function heldButtons(
  pads: Iterable<PadState | null>,
  map: Readonly<Record<number, Button>>,
): Set<Button> {
  const held = new Set<Button>();

  for (const pad of pads) {
    if (!pad || pad.mapping !== "standard") continue;

    pad.buttons.forEach((button, index) => {
      // `pressed` rather than a threshold on `value`: the browser already applies the
      // right one per button, and the Game Boy has no analog inputs to preserve.
      if (!button.pressed) return;
      const mapped = map[index];
      if (mapped) held.add(mapped);
    });

    // The stick is a direction, not an index, so it is not part of `map`. Note the Y
    // sign: the Gamepad API reports +1 as *down*, the opposite of the convention the
    // desktop build's gilrs uses.
    addAxis(held, pad.axes[LEFT_STICK_X], "left", "right");
    addAxis(held, pad.axes[LEFT_STICK_Y], "up", "down");
  }

  cancelOpposites(held);
  return held;
}

/** Adds whichever direction an axis position is holding, if any. */
function addAxis(
  held: Set<Button>,
  value: number | undefined,
  negative: Button,
  positive: Button,
): void {
  if (value === undefined) return;
  if (value <= -STICK_THRESHOLD) held.add(negative);
  else if (value >= STICK_THRESHOLD) held.add(positive);
}

/**
 * Drops both halves of an opposing pair held at once.
 *
 * A physical d-pad cannot produce Left+Right — it is one rocker — and some games handle
 * the impossible state badly. Two sources can still ask for it here: a stick pushed one
 * way while the d-pad is held the other, or two pads disagreeing. Neutral is the
 * closest reachable state.
 */
function cancelOpposites(held: Set<Button>): void {
  const pairs: readonly (readonly [Button, Button])[] = [
    ["left", "right"],
    ["up", "down"],
  ];
  for (const [a, b] of pairs) {
    if (held.has(a) && held.has(b)) {
      held.delete(a);
      held.delete(b);
    }
  }
}

/**
 * The edges between two snapshots.
 *
 * Releases come first so that flicking a stick from Left straight through centre to
 * Right cannot leave Left stuck down for the instant the presses are applied.
 */
export function diff(previous: ReadonlySet<Button>, next: ReadonlySet<Button>): Action[] {
  const actions: Action[] = [];
  for (const button of previous) {
    if (!next.has(button)) actions.push({ type: "release", button });
  }
  for (const button of next) {
    if (!previous.has(button)) actions.push({ type: "press", button });
  }
  return actions;
}

/** Remembers the last snapshot so each poll can report only what changed. */
export class GamepadInput {
  private readonly map: Readonly<Record<number, Button>>;
  private held: ReadonlySet<Button> = new Set();

  constructor(map: Readonly<Record<number, Button>> = DEFAULT_GAMEPAD_MAP) {
    this.map = map;
  }

  /** Diffs the pads against the previous poll and returns the edges to apply. */
  poll(pads: Iterable<PadState | null>): Action[] {
    const next = heldButtons(pads, this.map);
    const actions = diff(this.held, next);
    this.held = next;
    return actions;
  }

  /**
   * Releases everything currently held, forgetting the snapshot.
   *
   * Used when the loop stops: a button held at the moment of a pause would otherwise
   * still be held from the emulator's point of view, with no poll coming to release it.
   */
  releaseAll(): Action[] {
    const actions = diff(this.held, new Set());
    this.held = new Set();
    return actions;
  }
}
