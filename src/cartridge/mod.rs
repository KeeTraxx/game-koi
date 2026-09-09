//! Loading cartridges from ROM files.
//!
//! For now a [`Cartridge`] is just the ROM image plus its parsed header. Once the
//! memory bus exists, this is where mapper behavior (bank switching, RAM enable)
//! will live, and the bus will reach the ROM only through this type.

pub mod header;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use header::{Header, HeaderError};

/// A loaded cartridge.
///
/// The ROM bytes and path are stored but not read yet; the memory bus will consume
/// them once it exists.
#[allow(dead_code)]
pub struct Cartridge {
    /// The full ROM image, exactly as it was on the file.
    rom: Vec<u8>,
    header: Header,
    /// Where it was loaded from, for error messages and later save-file naming.
    path: PathBuf,
}

/// Anything that can go wrong loading a ROM file.
#[derive(Debug)]
pub enum LoadError {
    Io { path: PathBuf, source: io::Error },
    Header { path: PathBuf, source: HeaderError },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            LoadError::Header { path, source } => {
                write!(
                    f,
                    "invalid cartridge header in {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io { source, .. } => Some(source),
            LoadError::Header { source, .. } => Some(source),
        }
    }
}

#[allow(dead_code)]
impl Cartridge {
    /// Reads a ROM file from disk and parses its header.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let path = path.as_ref();
        let rom = fs::read(path).map_err(|source| LoadError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(rom, path)
    }

    /// Builds a cartridge from an in-memory ROM image.
    ///
    /// Split out from [`Cartridge::load`] so tests can construct one without
    /// touching the filesystem.
    pub fn from_bytes(rom: Vec<u8>, path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let path = path.as_ref().to_path_buf();
        let header = Header::parse(&rom).map_err(|source| LoadError::Header {
            path: path.clone(),
            source,
        })?;
        Ok(Cartridge { rom, header, path })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The raw ROM image.
    ///
    /// Temporary: once the bus exists it should go through mapper-aware reads
    /// instead, since anything past bank 0 depends on the current bank registers.
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_missing_file() {
        let Err(err) = Cartridge::load("/nonexistent/nope.gb") else {
            panic!("expected a load failure");
        };
        assert!(matches!(err, LoadError::Io { .. }));
        assert!(err.to_string().contains("nope.gb"));
    }

    #[test]
    fn reports_a_bad_header_with_the_path() {
        let Err(err) = Cartridge::from_bytes(vec![0u8; 64], "bad.gb") else {
            panic!("expected a header failure");
        };
        assert!(matches!(err, LoadError::Header { .. }));
        assert!(err.to_string().contains("bad.gb"));
    }
}
