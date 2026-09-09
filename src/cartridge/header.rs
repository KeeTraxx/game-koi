//! Parsing of the cartridge header at 0x0100-0x014F.
//!
//! Every Game Boy ROM starts with a fixed-layout header describing the game: its
//! title, which mapper chip ("MBC") the cartridge board uses, and how much ROM and
//! RAM is on the board. The real boot ROM reads part of this area to decide whether
//! to boot at all; we read it to decide how to wire up the cartridge.

use std::fmt;

/// Offsets into the ROM. These are absolute addresses in bank 0, which is mapped at
/// 0x0000, so they double as indices into the ROM file itself.
mod offset {
    pub const LOGO: usize = 0x0104;
    pub const TITLE: usize = 0x0134;
    pub const NEW_LICENSEE: usize = 0x0144;
    pub const SGB_FLAG: usize = 0x0146;
    pub const CARTRIDGE_TYPE: usize = 0x0147;
    pub const ROM_SIZE: usize = 0x0148;
    pub const RAM_SIZE: usize = 0x0149;
    pub const DESTINATION: usize = 0x014A;
    pub const OLD_LICENSEE: usize = 0x014B;
    pub const VERSION: usize = 0x014C;
    pub const HEADER_CHECKSUM: usize = 0x014D;
    pub const GLOBAL_CHECKSUM: usize = 0x014E;

    /// One past the last header byte. A ROM shorter than this has no header.
    pub const HEADER_END: usize = 0x0150;
}

/// The Nintendo logo bitmap stored at 0x0104-0x0133.
///
/// The boot ROM compares the cartridge's copy against its own and halts forever if
/// they differ — this was the copy-protection scheme, since shipping the bytes meant
/// reproducing Nintendo's trademarked logo. We only use it as a sanity check that we
/// are actually looking at a Game Boy ROM.
const NINTENDO_LOGO: [u8; 48] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
    0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
    0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

/// Which mapper chip the cartridge board carries, and what else is on the board.
///
/// The mapper sits between the CPU and the ROM chip and handles bank switching: the
/// CPU can only address 32 KiB of cartridge ROM at once, so anything larger needs
/// hardware to swap which 16 KiB chunk appears at 0x4000-0x7FFF. Writes into the ROM
/// address range don't change memory — they're commands to this chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapper {
    /// No mapper at all: up to 32 KiB of ROM wired straight to the bus.
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
    Mbc6,
    Mbc7,
    Mmm01,
    HuC1,
    HuC3,
    PocketCamera,
    Tama5,
}

/// The decoded contents of byte 0x0147, which packs the mapper together with flags
/// for the optional extras on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CartridgeType {
    pub mapper: Mapper,
    /// Board has RAM in the 0xA000-0xBFFF window.
    pub has_ram: bool,
    /// RAM is backed by a battery, so saves survive power-off.
    pub has_battery: bool,
    /// Board has a timer chip (MBC3's real-time clock).
    pub has_timer: bool,
    /// Board has a rumble motor (drives one of MBC5's bank register bits).
    pub has_rumble: bool,
    /// Board has an accelerometer (MBC7).
    pub has_sensor: bool,
}

impl CartridgeType {
    /// Decodes byte 0x0147. Returns `None` for unassigned values.
    ///
    /// This is a lookup table rather than bit decoding: the encoding grew by
    /// accretion as new boards shipped, so the values are not systematic.
    fn from_byte(byte: u8) -> Option<Self> {
        use Mapper::*;

        // (mapper, ram, battery, timer, rumble, sensor)
        let (mapper, ram, battery, timer, rumble, sensor) = match byte {
            0x00 => (None, false, false, false, false, false),
            0x01 => (Mbc1, false, false, false, false, false),
            0x02 => (Mbc1, true, false, false, false, false),
            0x03 => (Mbc1, true, true, false, false, false),
            // MBC2's RAM is built into the mapper chip itself, so the type byte
            // has no separate "+RAM" variant: it is always present.
            0x05 => (Mbc2, true, false, false, false, false),
            0x06 => (Mbc2, true, true, false, false, false),
            0x08 => (None, true, false, false, false, false),
            0x09 => (None, true, true, false, false, false),
            0x0B => (Mmm01, false, false, false, false, false),
            0x0C => (Mmm01, true, false, false, false, false),
            0x0D => (Mmm01, true, true, false, false, false),
            0x0F => (Mbc3, false, true, true, false, false),
            0x10 => (Mbc3, true, true, true, false, false),
            0x11 => (Mbc3, false, false, false, false, false),
            0x12 => (Mbc3, true, false, false, false, false),
            0x13 => (Mbc3, true, true, false, false, false),
            0x19 => (Mbc5, false, false, false, false, false),
            0x1A => (Mbc5, true, false, false, false, false),
            0x1B => (Mbc5, true, true, false, false, false),
            0x1C => (Mbc5, false, false, false, true, false),
            0x1D => (Mbc5, true, false, false, true, false),
            0x1E => (Mbc5, true, true, false, true, false),
            0x20 => (Mbc6, true, true, false, false, false),
            0x22 => (Mbc7, true, true, false, true, true),
            0xFC => (PocketCamera, true, true, false, false, false),
            0xFD => (Tama5, true, true, false, false, false),
            0xFE => (HuC3, false, false, false, false, false),
            0xFF => (HuC1, true, true, false, false, false),
            _ => return Option::None,
        };

        Some(CartridgeType {
            mapper,
            has_ram: ram,
            has_battery: battery,
            has_timer: timer,
            has_rumble: rumble,
            has_sensor: sensor,
        })
    }
}

/// Things that can be wrong with a ROM file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// File is too short to even contain a header.
    TooShort { len: usize },
    /// The logo bytes at 0x0104-0x0133 don't match — probably not a Game Boy ROM.
    BadLogo,
    /// Byte 0x0147 holds a value with no assigned meaning.
    UnknownCartridgeType(u8),
    /// Byte 0x0148 holds a value with no assigned meaning.
    UnknownRomSize(u8),
    /// Byte 0x0149 holds a value with no assigned meaning.
    UnknownRamSize(u8),
    /// The file's length disagrees with the size declared in the header.
    RomSizeMismatch { declared: usize, actual: usize },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderError::TooShort { len } => write!(
                f,
                "file is {len} bytes, too short to contain a cartridge header \
                 (need at least 0x{:04X})",
                offset::HEADER_END
            ),
            HeaderError::BadLogo => {
                write!(
                    f,
                    "Nintendo logo at 0x0104-0x0133 is wrong; not a Game Boy ROM?"
                )
            }
            HeaderError::UnknownCartridgeType(b) => {
                write!(f, "unknown cartridge type 0x{b:02X} at 0x0147")
            }
            HeaderError::UnknownRomSize(b) => write!(f, "unknown ROM size 0x{b:02X} at 0x0148"),
            HeaderError::UnknownRamSize(b) => write!(f, "unknown RAM size 0x{b:02X} at 0x0149"),
            HeaderError::RomSizeMismatch { declared, actual } => write!(
                f,
                "header declares {declared} bytes of ROM but the file is {actual} bytes"
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

/// A parsed cartridge header.
///
/// Some fields are not read by anything yet; they are parsed now because the header
/// is a fixed layout and it is less confusing to decode it in one pass.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Header {
    /// Game title from 0x0134, trimmed of padding. Later cartridges shortened this
    /// field to make room for the manufacturer code and CGB flag, so we stop at the
    /// first NUL or non-ASCII byte rather than assuming a fixed length.
    pub title: String,
    pub cartridge_type: CartridgeType,
    /// Total ROM size in bytes, decoded from 0x0148.
    pub rom_size: usize,
    /// Total cartridge RAM size in bytes, decoded from 0x0149.
    pub ram_size: usize,
    /// Whether the cartridge claims Super Game Boy support (0x0146 == 0x03).
    pub sgb_support: bool,
    /// Sold in Japan (0x014A == 0x00) or overseas.
    pub japanese: bool,
    /// Publisher code. Cartridges made from 1998 on set 0x014B to 0x33 and put a
    /// two-character ASCII code at 0x0144; older ones use a single byte at 0x014B.
    pub licensee: String,
    /// Version number of the game, from 0x014C. Usually 0.
    pub version: u8,
    /// Checksum byte at 0x014D and whether it matches what we computed.
    pub header_checksum: u8,
    pub header_checksum_valid: bool,
    /// Big-endian checksum at 0x014E-0x014F and whether it matches.
    ///
    /// Nothing on real hardware verifies this one, and some official games ship with
    /// it wrong, so a mismatch is informational only.
    pub global_checksum: u16,
    pub global_checksum_valid: bool,
}

impl Header {
    /// Parses the header out of a full ROM image.
    ///
    /// Rejects a ROM whose declared size disagrees with the file length, since every
    /// later assumption about bank numbering depends on that size being truthful.
    /// The two checksums are recorded rather than enforced — see the field docs.
    pub fn parse(rom: &[u8]) -> Result<Self, HeaderError> {
        if rom.len() < offset::HEADER_END {
            return Err(HeaderError::TooShort { len: rom.len() });
        }

        if !has_logo_at(rom, 0) {
            return Err(HeaderError::BadLogo);
        }

        let type_byte = rom[offset::CARTRIDGE_TYPE];
        let cartridge_type = CartridgeType::from_byte(type_byte)
            .ok_or(HeaderError::UnknownCartridgeType(type_byte))?;

        let rom_size_byte = rom[offset::ROM_SIZE];
        let rom_size =
            decode_rom_size(rom_size_byte).ok_or(HeaderError::UnknownRomSize(rom_size_byte))?;
        if rom_size != rom.len() {
            return Err(HeaderError::RomSizeMismatch {
                declared: rom_size,
                actual: rom.len(),
            });
        }

        let ram_size_byte = rom[offset::RAM_SIZE];
        let ram_size = decode_ram_size(ram_size_byte, cartridge_type.mapper)
            .ok_or(HeaderError::UnknownRamSize(ram_size_byte))?;

        let header_checksum = rom[offset::HEADER_CHECKSUM];
        let global_checksum = u16::from_be_bytes([
            rom[offset::GLOBAL_CHECKSUM],
            rom[offset::GLOBAL_CHECKSUM + 1],
        ]);

        Ok(Header {
            title: parse_title(&rom[offset::TITLE..offset::NEW_LICENSEE]),
            cartridge_type,
            rom_size,
            ram_size,
            sgb_support: rom[offset::SGB_FLAG] == 0x03,
            japanese: rom[offset::DESTINATION] == 0x00,
            licensee: parse_licensee(rom),
            version: rom[offset::VERSION],
            header_checksum,
            header_checksum_valid: compute_header_checksum(rom) == header_checksum,
            global_checksum,
            global_checksum_valid: compute_global_checksum(rom) == global_checksum,
        })
    }

    /// Number of 16 KiB banks in the ROM.
    pub fn rom_banks(&self) -> usize {
        self.rom_size / 0x4000
    }

    /// Number of 8 KiB banks of cartridge RAM. Zero if the board has no RAM.
    pub fn ram_banks(&self) -> usize {
        self.ram_size.div_ceil(0x2000)
    }
}

/// Whether a correct Nintendo logo sits at `base + 0x0104`.
///
/// `base` is a physical offset into the image, not an address the CPU can see. It is 0
/// for the cartridge's own header, but MBC1 multicarts carry a full header per bundled
/// game, and finding several logos at 256 KiB intervals is the only way to recognize
/// one — the type byte is indistinguishable from a plain MBC1. See `mbc::mbc1`.
pub fn has_logo_at(rom: &[u8], base: usize) -> bool {
    let start = base + offset::LOGO;
    let end = start + NINTENDO_LOGO.len();
    rom.len() >= end && rom[start..end] == NINTENDO_LOGO
}

/// Stamps a Nintendo logo at `base + 0x0104`.
///
/// Test-only companion to [`has_logo_at`], so tests elsewhere can build an image that
/// looks like a multicart without needing a copy of the logo bytes.
#[cfg(test)]
pub fn write_logo_at(rom: &mut [u8], base: usize) {
    let start = base + offset::LOGO;
    rom[start..start + NINTENDO_LOGO.len()].copy_from_slice(&NINTENDO_LOGO);
}

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let title = if self.title.is_empty() {
            "<untitled>"
        } else {
            &self.title
        };
        writeln!(f, "Title:     {title}")?;
        writeln!(f, "Licensee:  {}", self.licensee)?;
        writeln!(f, "Version:   {}", self.version)?;
        writeln!(f, "Mapper:    {:?}", self.cartridge_type.mapper)?;
        writeln!(
            f,
            "ROM:       {} KiB ({} banks)",
            self.rom_size / 1024,
            self.rom_banks()
        )?;
        if self.ram_size == 0 {
            writeln!(f, "RAM:       none")?;
        } else {
            writeln!(
                f,
                "RAM:       {} KiB ({} banks){}",
                self.ram_size / 1024,
                self.ram_banks(),
                if self.cartridge_type.has_battery {
                    ", battery-backed"
                } else {
                    ""
                }
            )?;
        }
        writeln!(
            f,
            "Region:    {}",
            if self.japanese { "Japan" } else { "non-Japan" }
        )?;
        writeln!(
            f,
            "Checksums: header {}, global {}",
            if self.header_checksum_valid {
                "ok"
            } else {
                "BAD"
            },
            if self.global_checksum_valid {
                "ok"
            } else {
                "BAD"
            },
        )
    }
}

/// Decodes byte 0x0148 into a byte count.
///
/// The value is a shift count: the ROM is `32 KiB << n`. Values above 0x08 are not
/// assigned; a few unofficial 0x52-0x54 codes exist but no genuine cartridge uses
/// them, so we reject them rather than guess.
fn decode_rom_size(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x08 => Some((32 * 1024) << byte),
        _ => None,
    }
}

/// Decodes byte 0x0149 into a byte count.
///
/// Two wrinkles. Code 0x01 was never used by a real cartridge and is treated as no
/// RAM. And MBC2 carries 512 half-bytes of RAM inside the mapper chip itself, which
/// this field does not describe — those cartridges declare 0x00 here, so we
/// substitute the real size.
fn decode_ram_size(byte: u8, mapper: Mapper) -> Option<usize> {
    if mapper == Mapper::Mbc2 {
        // 512 x 4 bits. We give each nibble a full byte for simplicity; the mapper
        // is responsible for masking the high nibble off on read.
        return match byte {
            0x00 => Some(512),
            _ => None,
        };
    }

    match byte {
        0x00 => Some(0),
        0x01 => Some(0),
        0x02 => Some(8 * 1024),
        0x03 => Some(32 * 1024),
        0x04 => Some(128 * 1024),
        0x05 => Some(64 * 1024),
        _ => None,
    }
}

/// Reads the title field, stopping at the first NUL or non-ASCII byte.
///
/// The field was originally 16 bytes of upper-case ASCII padded with NULs, but later
/// cartridges reused its tail for the manufacturer code and CGB flag, so a raw
/// 16-byte read can pick up binary garbage.
fn parse_title(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0 && b.is_ascii() && !b.is_ascii_control())
        .map(|&b| b as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Reads the publisher code, preferring the newer two-character form.
fn parse_licensee(rom: &[u8]) -> String {
    let old = rom[offset::OLD_LICENSEE];
    if old == 0x33 {
        let bytes = &rom[offset::NEW_LICENSEE..offset::NEW_LICENSEE + 2];
        if bytes.iter().all(|b| b.is_ascii_graphic()) {
            return String::from_utf8_lossy(bytes).into_owned();
        }
    }
    format!("{old:02X}")
}

/// Computes the header checksum over 0x0134-0x014C.
///
/// The boot ROM runs exactly this loop and refuses to start the game if the result
/// disagrees with byte 0x014D, so every commercial cartridge has a correct one. Note
/// the subtraction: `x = x - byte - 1` for each byte, with wrapping.
fn compute_header_checksum(rom: &[u8]) -> u8 {
    rom[offset::TITLE..offset::HEADER_CHECKSUM]
        .iter()
        .fold(0u8, |acc, &byte| acc.wrapping_sub(byte).wrapping_sub(1))
}

/// Computes the global checksum: a plain 16-bit sum of every byte in the ROM except
/// the two checksum bytes themselves.
fn compute_global_checksum(rom: &[u8]) -> u16 {
    rom.iter()
        .enumerate()
        .filter(|(i, _)| *i != offset::GLOBAL_CHECKSUM && *i != offset::GLOBAL_CHECKSUM + 1)
        .fold(0u16, |acc, (_, &byte)| acc.wrapping_add(byte as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic ROM: 32 KiB, valid logo, with the header checksum
    /// fixed up so it verifies.
    fn test_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 32 * 1024];
        rom[offset::LOGO..offset::LOGO + NINTENDO_LOGO.len()].copy_from_slice(&NINTENDO_LOGO);
        rom[offset::TITLE..offset::TITLE + 6].copy_from_slice(b"TETRIS");
        rom[offset::CARTRIDGE_TYPE] = 0x00;
        rom[offset::ROM_SIZE] = 0x00;
        rom[offset::RAM_SIZE] = 0x00;
        rom[offset::OLD_LICENSEE] = 0x01;
        fix_checksums(&mut rom);
        rom
    }

    fn fix_checksums(rom: &mut [u8]) {
        rom[offset::HEADER_CHECKSUM] = compute_header_checksum(rom);
        let global = compute_global_checksum(rom).to_be_bytes();
        rom[offset::GLOBAL_CHECKSUM] = global[0];
        rom[offset::GLOBAL_CHECKSUM + 1] = global[1];
    }

    #[test]
    fn parses_a_minimal_rom() {
        let header = Header::parse(&test_rom()).unwrap();
        assert_eq!(header.title, "TETRIS");
        assert_eq!(header.cartridge_type.mapper, Mapper::None);
        assert_eq!(header.rom_size, 32 * 1024);
        assert_eq!(header.rom_banks(), 2);
        assert_eq!(header.ram_size, 0);
        assert_eq!(header.licensee, "01");
        assert!(header.header_checksum_valid);
        assert!(header.global_checksum_valid);
    }

    #[test]
    fn rejects_short_files() {
        assert_eq!(
            Header::parse(&[0u8; 16]).unwrap_err(),
            HeaderError::TooShort { len: 16 }
        );
    }

    #[test]
    fn rejects_a_corrupt_logo() {
        let mut rom = test_rom();
        rom[offset::LOGO] ^= 0xFF;
        assert_eq!(Header::parse(&rom).unwrap_err(), HeaderError::BadLogo);
    }

    #[test]
    fn rejects_unknown_cartridge_type() {
        let mut rom = test_rom();
        rom[offset::CARTRIDGE_TYPE] = 0x04;
        fix_checksums(&mut rom);
        assert_eq!(
            Header::parse(&rom).unwrap_err(),
            HeaderError::UnknownCartridgeType(0x04)
        );
    }

    #[test]
    fn rejects_size_mismatch() {
        let mut rom = test_rom();
        // Claim 64 KiB while the file stays 32 KiB.
        rom[offset::ROM_SIZE] = 0x01;
        fix_checksums(&mut rom);
        assert_eq!(
            Header::parse(&rom).unwrap_err(),
            HeaderError::RomSizeMismatch {
                declared: 64 * 1024,
                actual: 32 * 1024,
            }
        );
    }

    #[test]
    fn reports_bad_header_checksum_without_failing() {
        let mut rom = test_rom();
        rom[offset::HEADER_CHECKSUM] = rom[offset::HEADER_CHECKSUM].wrapping_add(1);
        let header = Header::parse(&rom).unwrap();
        assert!(!header.header_checksum_valid);
    }

    #[test]
    fn header_checksum_matches_known_value() {
        // Worked by hand from the boot ROM's algorithm: with every byte in
        // 0x0134-0x014C zero, the result is -(0x19 bytes) = 0x100 - 0x19.
        let mut rom = vec![0u8; 32 * 1024];
        rom[offset::LOGO..offset::LOGO + NINTENDO_LOGO.len()].copy_from_slice(&NINTENDO_LOGO);
        assert_eq!(compute_header_checksum(&rom), 0u8.wrapping_sub(0x19));
    }

    #[test]
    fn decodes_rom_sizes() {
        assert_eq!(decode_rom_size(0x00), Some(32 * 1024));
        assert_eq!(decode_rom_size(0x01), Some(64 * 1024));
        assert_eq!(decode_rom_size(0x08), Some(8 * 1024 * 1024));
        assert_eq!(decode_rom_size(0x09), None);
    }

    #[test]
    fn decodes_ram_sizes() {
        assert_eq!(decode_ram_size(0x00, Mapper::None), Some(0));
        assert_eq!(decode_ram_size(0x02, Mapper::Mbc1), Some(8 * 1024));
        // Note 0x05 is 64 KiB and 0x04 is 128 KiB — the codes are not in order.
        assert_eq!(decode_ram_size(0x04, Mapper::Mbc1), Some(128 * 1024));
        assert_eq!(decode_ram_size(0x05, Mapper::Mbc1), Some(64 * 1024));
        assert_eq!(decode_ram_size(0x06, Mapper::Mbc1), None);
        // MBC2's RAM is internal and not described by this field.
        assert_eq!(decode_ram_size(0x00, Mapper::Mbc2), Some(512));
    }

    #[test]
    fn decodes_cartridge_type_flags() {
        let t = CartridgeType::from_byte(0x1B).unwrap();
        assert_eq!(t.mapper, Mapper::Mbc5);
        assert!(t.has_ram && t.has_battery);
        assert!(!t.has_rumble);

        let t = CartridgeType::from_byte(0x10).unwrap();
        assert_eq!(t.mapper, Mapper::Mbc3);
        assert!(t.has_timer && t.has_battery && t.has_ram);
    }

    #[test]
    fn stops_title_at_binary_padding() {
        let mut rom = test_rom();
        // Emulate a later cartridge that put a CGB flag right after a short title.
        rom[offset::TITLE + 6] = 0x80;
        fix_checksums(&mut rom);
        assert_eq!(Header::parse(&rom).unwrap().title, "TETRIS");
    }

    #[test]
    fn prefers_new_licensee_code() {
        let mut rom = test_rom();
        rom[offset::OLD_LICENSEE] = 0x33;
        rom[offset::NEW_LICENSEE] = b'0';
        rom[offset::NEW_LICENSEE + 1] = b'1';
        fix_checksums(&mut rom);
        assert_eq!(Header::parse(&rom).unwrap().licensee, "01");
    }
}
