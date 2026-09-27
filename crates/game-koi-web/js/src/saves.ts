/**
 * Battery-backed saves, kept in `localStorage`.
 *
 * The browser counterpart of the desktop's `save.rs`. The rule for *what* is saved
 * lives on the Rust side (`Emulator::has_save`: a battery and RAM to keep powered);
 * this module only decides where the bytes go and what they are called.
 *
 * # The key
 *
 * The desktop names a save after the ROM's filename, but a page hands over bytes, not
 * a file. The header title is not enough on its own — plenty of cartridges share one,
 * and a ROM hack keeps its original's — so the key is the title plus a CRC32 of the
 * whole ROM, the same checksum No-Intro uses to identify dumps. The title is only
 * there so a person reading their storage in devtools can tell what is what.
 *
 * # The encoding
 *
 * `localStorage` holds strings, so the RAM is stored as base64. The largest DMG save
 * is 128 KiB (MBC5), about 171 KiB encoded, against a per-origin quota of roughly
 * 5 MiB — ample for a handful of games, not for a library of the largest ones. A write
 * that hits the quota throws; the caller leaves the save dirty and tries again later.
 *
 * Everything but {@link LocalStorageSaves} is pure, and that class takes its `Storage`
 * as a parameter, so all of it is testable under Node without a DOM.
 */

const KEY_PREFIX = "game-koi:save:";

let crcTable: Uint32Array | null = null;

/** CRC-32 (IEEE 802.3, as zlib and No-Intro use), over the whole input. */
export function crc32(bytes: Uint8Array): number {
  if (!crcTable) {
    crcTable = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c >>> 0;
    }
  }
  let crc = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) {
    crc = crcTable[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/**
 * The cartridge title from the header, 0x134–0x143.
 *
 * Read up to the first NUL, printable ASCII only. On later cartridges the last bytes of
 * that range were repurposed (manufacturer code, CGB flag), which is why stray
 * non-letters get dropped rather than trusted.
 */
export function romTitle(rom: Uint8Array): string {
  let title = "";
  for (let i = 0x134; i < 0x144 && i < rom.length; i++) {
    const byte = rom[i];
    if (byte === 0) break;
    if (byte >= 0x20 && byte < 0x7f) title += String.fromCharCode(byte);
  }
  return title.trim();
}

/** The storage key a ROM's save lives under. */
export function saveKey(rom: Uint8Array): string {
  const crc = crc32(rom).toString(16).padStart(8, "0");
  return `${KEY_PREFIX}${romTitle(rom) || "untitled"}:${crc}`;
}

export function encodeSave(bytes: Uint8Array): string {
  // Chunked because `String.fromCharCode(...bytes)` on 128 KiB overflows the argument
  // limit of some engines' call stacks.
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/** Decodes a stored save, or `null` if the string is not valid base64. */
export function decodeSave(text: string): Uint8Array | null {
  let binary: string;
  try {
    binary = atob(text);
  } catch {
    return null;
  }
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** Reads and writes saves in a `Storage` — `localStorage` in a page. */
export class LocalStorageSaves {
  constructor(private readonly storage: Storage) {}

  load(key: string): Uint8Array | null {
    const text = this.storage.getItem(key);
    return text === null ? null : decodeSave(text);
  }

  /** Throws if the write fails — most often `QuotaExceededError`. */
  store(key: string, bytes: Uint8Array): void {
    this.storage.setItem(key, encodeSave(bytes));
  }
}

/**
 * `localStorage`, or `null` where touching it throws.
 *
 * It is not just absent in odd places: with storage blocked by the user, or in a
 * sandboxed iframe, merely *reading the property* throws a `SecurityError`. A page
 * that cannot save should still play.
 */
export function defaultStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}
