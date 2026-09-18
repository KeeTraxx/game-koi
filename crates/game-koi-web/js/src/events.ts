/**
 * The package's event plumbing: a tiny typed emitter, plus the button-edge types it
 * carries.
 *
 * `on`/`off` rather than `EventTarget`, because a `CustomEvent`'s payload lands in
 * `detail` typed as whatever the listener's declaration says — subclassing `EventTarget`
 * in TypeScript means writing `addEventListener` overloads by hand and consumers still
 * end up casting. A five-line map of sets types exactly, costs nothing, and keeps the
 * package free of DOM globals in a module that Node's test runner has to import.
 *
 * The emitter deals in *edges*, not state: `GameKoi` holds the set of what is down and
 * only emits when a button actually changes, so a key held with autorepeat, a stick
 * resting past the threshold, and a page calling `press` twice all produce one event.
 */

import type { Button } from "./index.js";

/** Where a button edge came from. */
export type ButtonSource = "keyboard" | "gamepad" | "api";

/** A button going down or coming up. */
export interface ButtonEvent {
  button: Button;
  /**
   * Which input path produced this edge. `"api"` covers `press`/`release` called by the
   * page, and the releases `loadRom()` emits for a machine that is going away;
   * `pause()`'s releases are attributed to the device that was holding them.
   */
  source: ButtonSource;
  /**
   * Everything held *after* this edge — a snapshot, safe to keep.
   *
   * Handy for the common case of mirroring the joypad on screen: render from this and
   * there is no second copy of the state to keep in step.
   */
  pressed: ReadonlySet<Button>;
}

/** Event names to their payloads. See {@link GameKoi.on}. */
export interface GameKoiEvents {
  press: ButtonEvent;
  release: ButtonEvent;
}

export type Listener<E> = (event: E) => void;

/** Unsubscribes the listener it was returned for. Calling it twice is harmless. */
export type Unsubscribe = () => void;

/** A minimal typed event emitter. */
export class Emitter<Events> {
  private readonly listeners = new Map<keyof Events, Set<Listener<never>>>();

  /** Registers a listener, and returns a function that removes it again. */
  on<K extends keyof Events>(type: K, listener: Listener<Events[K]>): Unsubscribe {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(listener as Listener<never>);
    return () => this.off(type, listener);
  }

  off<K extends keyof Events>(type: K, listener: Listener<Events[K]>): void {
    this.listeners.get(type)?.delete(listener as Listener<never>);
  }

  /**
   * Calls every listener for `type`.
   *
   * Each is isolated: these fire from inside a `keydown` handler and from the render
   * loop, so a listener that throws would otherwise skip the rest of a gamepad poll or
   * kill the loop outright. A page's display bug should not stop the emulator, so the
   * error is reported and the remaining listeners still run.
   *
   * Iterates a copy, so a listener that unsubscribes itself (or another) mid-dispatch
   * cannot disturb the walk.
   */
  emit<K extends keyof Events>(type: K, event: Events[K]): void {
    const set = this.listeners.get(type);
    if (!set) return;
    for (const listener of [...set] as Listener<Events[K]>[]) {
      try {
        listener(event);
      } catch (error) {
        console.error(`game-koi: "${String(type)}" listener threw`, error);
      }
    }
  }

  /** Drops every listener. Called from `dispose()`. */
  clear(): void {
    this.listeners.clear();
  }
}
