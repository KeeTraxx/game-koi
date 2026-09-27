// Tests for the save helpers. All pure: the storage is passed in, so a Map-backed
// stand-in replaces `localStorage` and nothing here needs a browser.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  LocalStorageSaves,
  crc32,
  decodeSave,
  encodeSave,
  romTitle,
  saveKey,
} from "../dist/saves.js";

/** The minimal `Storage` surface `LocalStorageSaves` uses. */
class MemoryStorage {
  items = new Map();
  getItem(key) {
    return this.items.has(key) ? this.items.get(key) : null;
  }
  setItem(key, value) {
    this.items.set(key, String(value));
  }
}

/** A ROM-sized buffer with `title` in the header's title field. */
function romWithTitle(title, size = 0x8000) {
  const rom = new Uint8Array(size);
  for (let i = 0; i < title.length; i++) rom[0x134 + i] = title.charCodeAt(i);
  return rom;
}

test("crc32 matches the standard check value", () => {
  // "123456789" -> 0xCBF43926 is the published check value for CRC-32/IEEE.
  assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
  assert.equal(crc32(new Uint8Array()), 0);
});

test("romTitle stops at the first NUL and drops non-printables", () => {
  assert.equal(romTitle(romWithTitle("TETRIS")), "TETRIS");
  const rom = romWithTitle("POKEMON RED");
  rom[0x143] = 0x80; // CGB flag on later carts: not part of the title
  assert.equal(romTitle(rom), "POKEMON RED");
  assert.equal(romTitle(new Uint8Array(0x100)), "");
});

test("saveKey tells apart ROMs that share a title", () => {
  const original = romWithTitle("TETRIS");
  const hack = romWithTitle("TETRIS");
  hack[0x4000] = 1;
  assert.match(saveKey(original), /^game-koi:save:TETRIS:[0-9a-f]{8}$/);
  assert.notEqual(saveKey(original), saveKey(hack));
  assert.equal(saveKey(original), saveKey(romWithTitle("TETRIS")));
  assert.match(saveKey(new Uint8Array(0x8000)), /:untitled:/);
});

test("a save round-trips through base64, including the largest DMG RAM", () => {
  // 128 KiB is MBC5's maximum, and larger than one String.fromCharCode chunk.
  const ram = new Uint8Array(128 * 1024);
  for (let i = 0; i < ram.length; i++) ram[i] = (i * 31) & 0xff;
  assert.deepEqual(decodeSave(encodeSave(ram)), ram);
  assert.deepEqual(decodeSave(encodeSave(new Uint8Array())), new Uint8Array());
});

test("decodeSave refuses a corrupt entry rather than throwing", () => {
  assert.equal(decodeSave("not base64!"), null);
});

test("LocalStorageSaves stores and loads by key", () => {
  const storage = new MemoryStorage();
  const saves = new LocalStorageSaves(storage);
  assert.equal(saves.load("k"), null);
  saves.store("k", Uint8Array.of(1, 2, 3));
  assert.deepEqual(saves.load("k"), Uint8Array.of(1, 2, 3));
  assert.equal(typeof storage.getItem("k"), "string");
});
