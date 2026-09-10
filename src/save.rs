//! Battery-backed saves: keeping cartridge RAM between runs.
//!
//! A cartridge with a battery on its board keeps its save RAM powered when the console
//! is off — that is all "battery-backed" means, and it is the only reason a Game Boy
//! game can remember anything. A cartridge *without* one has the same RAM and loses it
//! the moment the power goes, so persisting it here would be wrong, not generous: see
//! [`SaveFile::for_cartridge`].
//!
//! # Where the files go
//!
//! `dirs::data_dir()`, plus `game-koi/`. That resolves to the place each platform
//! actually wants application data:
//!
//! | Platform | Directory |
//! |---|---|
//! | Linux | `$XDG_DATA_HOME`, else `~/.local/share` |
//! | macOS | `~/Library/Application Support` |
//! | Windows | `%APPDATA%` (Roaming) |
//!
//! Data rather than config: a save is generated state, not something a person edits.
//! On Linux that is the difference between `~/.local/share` and `~/.config`, and it is
//! also why these files should survive a config reset.
//!
//! Files are named after the ROM's filename, not its header title — plenty of
//! cartridges share a title field, and the filename is what the person actually chose.
//!
//! # What is not saved
//!
//! MBC3's real-time clock. On a real cartridge the same battery powers both the RAM and
//! the crystal, so a Pokémon Gold save that survives a power cycle also remembers what
//! day it was; here the clock restarts at zero every run. Other emulators append their
//! RTC state after the RAM, in formats that do not agree with each other, so adding it
//! means picking one. The loader already tolerates a longer file than it expects, which
//! is what a save written by one of those emulators looks like.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cartridge::Cartridge;
use crate::testbus::TestBus;

/// Where a cartridge's save RAM lives on disk.
pub struct SaveFile {
    path: PathBuf,
}

impl SaveFile {
    /// The save file for a cartridge, or `None` if it should not have one.
    ///
    /// Three ways to get `None`, and they are all correct rather than failures:
    /// the board has no battery, so its RAM is volatile on real hardware too; the board
    /// has no RAM at all; or the platform has no data directory to write to, which is
    /// odd but not a reason to refuse to play.
    pub fn for_cartridge(cart: &Cartridge) -> Option<Self> {
        let header = cart.header();
        if !header.cartridge_type.has_battery {
            return None;
        }
        // MBC2 is the exception to trusting the header here: its RAM is inside the
        // mapper chip, so the header's RAM-size byte is 0 even though there is RAM to
        // save. Ask the mapper instead of the header.
        let has_ram = cart
            .create_mbc()
            .ok()
            .and_then(|mbc| mbc.ram().map(|ram| !ram.bytes().is_empty()))
            .unwrap_or(false);
        if !has_ram {
            return None;
        }

        let stem = cart.path().file_stem()?;
        let mut path = dirs::data_dir()?;
        path.push("game-koi");
        path.push(stem);
        path.set_extension("sav");
        Some(SaveFile { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the save into the cartridge's RAM, if a save exists.
    ///
    /// A missing file is the normal first-run case, not an error. A file that is too
    /// short is refused: it is more likely a save for a different game than a truncated
    /// one for this game, and half-loading it would corrupt what the player has.
    pub fn load(&self, bus: &mut TestBus) -> io::Result<bool> {
        let data = match fs::read(&self.path) {
            Ok(data) => data,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err),
        };

        let Some(ram) = bus.cartridge_ram_mut() else {
            return Ok(false);
        };

        if data.len() < ram.bytes().len() {
            eprintln!(
                "warning: {} holds {} bytes but this cartridge has {}; ignoring it",
                self.path.display(),
                data.len(),
                ram.bytes().len()
            );
            return Ok(false);
        }

        ram.load(&data);
        Ok(true)
    }

    /// Writes the cartridge's RAM out.
    ///
    /// Writes to a temporary file and renames it into place, so that a crash or a power
    /// cut partway through leaves the previous save intact rather than a half-written
    /// one. `rename` is atomic within a filesystem on every platform we target — which
    /// is why the temporary sits in the same directory as the target and not in `/tmp`.
    pub fn store(&self, bus: &mut TestBus) -> io::Result<()> {
        let Some(ram) = bus.cartridge_ram_mut() else {
            return Ok(());
        };
        if ram.bytes().is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temporary = self.path.with_extension("sav.tmp");
        fs::write(&temporary, ram.bytes())?;
        fs::rename(&temporary, &self.path)?;

        ram.mark_clean();
        Ok(())
    }

    /// Writes the save only if the game has touched RAM since the last write.
    ///
    /// The autosave calls this every couple of seconds. Most of those calls do nothing,
    /// because a game only writes save RAM when it saves.
    pub fn store_if_dirty(&self, bus: &mut TestBus) -> io::Result<()> {
        let dirty = bus
            .cartridge_ram_mut()
            .map(|ram| ram.is_dirty())
            .unwrap_or(false);
        if dirty { self.store(bus) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;
    use crate::cartridge::header;

    /// A 32 KiB cartridge with the given type byte and RAM-size byte.
    fn cartridge(type_byte: u8, ram_size: u8, name: &str) -> Cartridge {
        let mut rom = vec![0u8; 32 * 1024];
        header::write_logo_at(&mut rom, 0);
        rom[0x0147] = type_byte;
        rom[0x0149] = ram_size;
        let checksum = rom[0x0134..0x014D]
            .iter()
            .fold(0u8, |acc, &byte| acc.wrapping_sub(byte).wrapping_sub(1));
        rom[0x014D] = checksum;
        Cartridge::from_bytes(rom, name).expect("test cartridge should parse")
    }

    #[test]
    fn a_cartridge_without_a_battery_gets_no_save_file() {
        // MBC1+RAM, no battery: on hardware this RAM dies with the power, so persisting
        // it would give the game a memory the real cartridge never had.
        let cart = cartridge(0x02, 0x02, "no-battery.gb");
        assert!(SaveFile::for_cartridge(&cart).is_none());
    }

    #[test]
    fn a_cartridge_without_ram_gets_no_save_file() {
        let cart = cartridge(0x01, 0x00, "no-ram.gb");
        assert!(SaveFile::for_cartridge(&cart).is_none());
    }

    #[test]
    fn a_battery_backed_cartridge_gets_one() {
        let cart = cartridge(0x03, 0x02, "zelda.gb");
        let save = SaveFile::for_cartridge(&cart).expect("battery means a save file");
        assert_eq!(save.path().extension().unwrap(), "sav");
        assert_eq!(save.path().file_stem().unwrap(), "zelda");
        assert!(
            save.path().parent().unwrap().ends_with("game-koi"),
            "saves belong in the application's own directory, got {}",
            save.path().display()
        );
    }

    #[test]
    fn mbc2_gets_a_save_despite_its_header_declaring_no_ram() {
        // The RAM is inside the mapper chip, so byte 0x0149 is 0. Trusting the header
        // here would silently lose every MBC2 game's saves.
        let cart = cartridge(0x06, 0x00, "mbc2-battery.gb");
        assert!(SaveFile::for_cartridge(&cart).is_some());
    }

    #[test]
    fn a_save_survives_a_round_trip() {
        let cart = cartridge(0x03, 0x02, "roundtrip.gb");
        let directory = std::env::temp_dir().join(format!("game-koi-test-{}", std::process::id()));
        let save = SaveFile {
            path: directory.join("roundtrip.sav"),
        };

        let mut bus = TestBus::new(&cart).expect("MBC1 is supported");
        bus.write(0x0000, 0x0A); // open the RAM gate, as a game saving would
        bus.write(0xA000, 0x42);
        bus.write(0xA001, 0x37);

        save.store(&mut bus).expect("store should succeed");

        let mut restored = TestBus::new(&cart).expect("MBC1 is supported");
        assert!(save.load(&mut restored).expect("load should succeed"));
        restored.write(0x0000, 0x0A);
        assert_eq!(restored.read(0xA000), 0x42);
        assert_eq!(restored.read(0xA001), 0x37);

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_missing_save_is_not_an_error() {
        let cart = cartridge(0x03, 0x02, "absent.gb");
        let save = SaveFile {
            path: std::env::temp_dir().join("game-koi-definitely-not-here.sav"),
        };
        let mut bus = TestBus::new(&cart).expect("MBC1 is supported");
        assert!(!save.load(&mut bus).expect("no file is fine"));
    }

    #[test]
    fn the_dirty_flag_tracks_writes() {
        let cart = cartridge(0x03, 0x02, "dirty.gb");
        let mut bus = TestBus::new(&cart).expect("MBC1 is supported");
        assert!(!bus.cartridge_ram_mut().unwrap().is_dirty());

        // A write through a shut gate changes nothing, so nothing is dirty.
        bus.write(0xA000, 0x42);
        assert!(!bus.cartridge_ram_mut().unwrap().is_dirty());

        bus.write(0x0000, 0x0A); // open the RAM gate, as a game saving would
        bus.write(0xA000, 0x42);
        assert!(bus.cartridge_ram_mut().unwrap().is_dirty());
    }
}
