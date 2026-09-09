//! The PPU (Picture Processing Unit).
//!
//! # The scanline state machine
//!
//! The PPU draws 144 visible scanlines, then spends 10 more lines' worth of time in
//! vertical blank — 154 lines total, each taking 456 T-cycles. That is 70224 T-cycles
//! per frame, or about 59.7 frames per second.
//!
//! Each visible line cycles through three modes, then VBlank takes over for the rest
//! of the frame:
//!
//! | Mode | Name        | Duration      | What it means                         |
//! |------|-------------|---------------|---------------------------------------|
//! | 2    | OAM scan    | 80 T-cycles   | Finding which sprites are on this line |
//! | 3    | Drawing     | 172+ T-cycles | Pushing pixels; VRAM is busy           |
//! | 0    | HBlank      | the remainder | Idle until the line's 456 are up       |
//! | 1    | VBlank      | 10 lines      | Idle; the frame is done                |
//!
//! Mode 3's real duration varies with sprite count and scroll position, stealing
//! time from mode 0. We use a fixed 172, which keeps the line at 456 total. That is
//! accurate enough for games but not for tests that measure mode timing precisely.
//!
//! # Why the mode matters beyond drawing
//!
//! The mode gates VRAM and OAM access: during mode 3 the PPU is reading VRAM, so the
//! CPU cannot, and reads return 0xFF. Games work around this by doing all their
//! drawing during VBlank — which is why the VBlank interrupt is the heartbeat that
//! nearly every Game Boy game is structured around.
//!
//! # Registers
//!
//! Bit layouts are from chapter 9 of the reference. Note the reference documents the
//! registers but not the tile data formats or mode timings; those follow the
//! standard behavior documented by Pandocs.

mod fetch;

use crate::interrupts::{Interrupt, InterruptState};

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

/// T-cycles in one scanline.
const CYCLES_PER_LINE: u32 = 456;
/// Scanlines per frame, including the 10 spent in VBlank.
const LINES_PER_FRAME: u8 = 154;
/// Duration of mode 2 (OAM scan).
const OAM_SCAN_CYCLES: u32 = 80;
/// Duration of mode 3 (drawing). Fixed here; real hardware varies it.
const DRAW_CYCLES: u32 = 172;

/// The PPU's current mode, which is also what STAT bits 0-1 report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Mode 0: idle for the rest of the line. VRAM and OAM are free.
    HBlank = 0,
    /// Mode 1: the frame is finished. Everything is free; this is when games draw.
    VBlank = 1,
    /// Mode 2: scanning OAM for sprites on this line. OAM is busy.
    OamScan = 2,
    /// Mode 3: pushing pixels. Both VRAM and OAM are busy.
    Drawing = 3,
}

/// One of the four shades a DMG can display.
///
/// The hardware has no colour: a pixel is two bits, and the palette registers map
/// those to shades. Index 0 is lightest.
pub type Shade = u8;

pub struct Ppu {
    /// 8 KiB of video RAM at 0x8000-0x9FFF: tile data and the two tile maps.
    vram: [u8; 0x2000],
    /// Object Attribute Memory at 0xFE00-0xFE9F: 40 sprites, 4 bytes each.
    oam: [u8; 0xA0],

    /// LCDC (0xFF40) — the master control register.
    lcdc: u8,
    /// STAT (0xFF41) — interrupt-enable bits plus the read-only mode and LYC flag.
    stat: u8,
    /// SCY/SCX (0xFF42-43) — where in the 256x256 background the screen sits.
    scy: u8,
    scx: u8,
    /// LY (0xFF44) — the scanline being drawn. Read-only.
    ly: u8,
    /// LYC (0xFF45) — compared against LY to raise a STAT interrupt.
    lyc: u8,
    /// BGP (0xFF47) — maps background colour indices to shades.
    bgp: u8,
    /// OBP0/OBP1 (0xFF48-49) — the two sprite palettes.
    obp0: u8,
    obp1: u8,
    /// WY/WX (0xFF4A-4B) — the window's position. WX is offset by 7.
    wy: u8,
    wx: u8,

    mode: Mode,
    /// T-cycles elapsed in the current scanline.
    line_cycles: u32,
    /// Counts scanlines on which the window was drawn. The window has its own
    /// counter because it does not scroll with the background: it advances only on
    /// lines where the window is actually visible.
    window_line: u8,

    /// The completed picture, one shade per pixel.
    framebuffer: [Shade; SCREEN_WIDTH * SCREEN_HEIGHT],
    /// Set when a frame completes, so the caller knows to present it.
    pub frame_ready: bool,
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            vram: [0; 0x2000],
            oam: [0; 0xA0],
            // Post-boot values: the LCD is on, showing the background.
            lcdc: 0x91,
            stat: 0x00,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
            mode: Mode::OamScan,
            line_cycles: 0,
            window_line: 0,
            framebuffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT],
            frame_ready: false,
        }
    }

    // --- LCDC bits, per register 9.1 ---

    fn lcd_enabled(&self) -> bool {
        self.lcdc & 0x80 != 0
    }
    /// WIN_MAP: which tile map the window uses.
    fn window_map_base(&self) -> u16 {
        if self.lcdc & 0x40 != 0 {
            0x1C00
        } else {
            0x1800
        }
    }
    fn window_enabled(&self) -> bool {
        self.lcdc & 0x20 != 0
    }
    /// TILE_SEL: which addressing mode background and window tiles use.
    fn signed_tile_addressing(&self) -> bool {
        self.lcdc & 0x10 == 0
    }
    /// BG_MAP: which tile map the background uses.
    fn bg_map_base(&self) -> u16 {
        if self.lcdc & 0x08 != 0 {
            0x1C00
        } else {
            0x1800
        }
    }
    /// OBJ_SIZE: sprites are 8x8 or 8x16.
    fn tall_sprites(&self) -> bool {
        self.lcdc & 0x04 != 0
    }
    fn sprites_enabled(&self) -> bool {
        self.lcdc & 0x02 != 0
    }
    /// BG_EN: on a DMG, clearing this blanks the background *and* the window.
    fn bg_enabled(&self) -> bool {
        self.lcdc & 0x01 != 0
    }

    /// Advances the PPU by one M-cycle (4 T-cycles).
    pub fn tick(&mut self, interrupts: &mut InterruptState) {
        if !self.lcd_enabled() {
            return;
        }

        self.line_cycles += 4;

        match self.mode {
            Mode::OamScan if self.line_cycles >= OAM_SCAN_CYCLES => {
                // Drawing is not interruptible and has no STAT interrupt of its own.
                self.mode = Mode::Drawing;
            }
            Mode::Drawing if self.line_cycles >= OAM_SCAN_CYCLES + DRAW_CYCLES => {
                // The line's pixels are produced in one go on entering HBlank.
                // Real hardware emits them progressively during mode 3; drawing the
                // whole line at once is indistinguishable unless a game changes
                // registers mid-line, which is a mid-scanline effect we don't model.
                self.draw_line();
                self.set_mode(Mode::HBlank, interrupts);
            }
            Mode::HBlank if self.line_cycles >= CYCLES_PER_LINE => {
                self.next_line(interrupts);
            }
            Mode::VBlank if self.line_cycles >= CYCLES_PER_LINE => {
                self.next_line(interrupts);
            }
            _ => {}
        }
    }

    /// Ends the current scanline and starts the next.
    fn next_line(&mut self, interrupts: &mut InterruptState) {
        self.line_cycles -= CYCLES_PER_LINE;
        self.ly += 1;

        if self.ly == SCREEN_HEIGHT as u8 {
            // The visible lines are done: the frame is complete.
            self.set_mode(Mode::VBlank, interrupts);
            interrupts.request(Interrupt::VBlank);
            self.frame_ready = true;
        } else if self.ly >= LINES_PER_FRAME {
            // Wrap around to the top of the next frame.
            self.ly = 0;
            self.window_line = 0;
            self.set_mode(Mode::OamScan, interrupts);
        } else if self.ly < SCREEN_HEIGHT as u8 {
            self.set_mode(Mode::OamScan, interrupts);
        }

        self.check_lyc(interrupts);
    }

    /// Changes mode and raises a STAT interrupt if the program asked for this one.
    ///
    /// STAT bits 3-5 (`INTR_M0`, `INTR_M1`, `INTR_M2`) each enable the interrupt for
    /// one mode. There is no interrupt for mode 3.
    fn set_mode(&mut self, mode: Mode, interrupts: &mut InterruptState) {
        self.mode = mode;

        let enabled = match mode {
            Mode::HBlank => self.stat & 0x08 != 0,
            Mode::VBlank => self.stat & 0x10 != 0,
            Mode::OamScan => self.stat & 0x20 != 0,
            Mode::Drawing => false,
        };
        if enabled {
            interrupts.request(Interrupt::Stat);
        }
    }

    /// Updates the LYC=LY flag and raises a STAT interrupt on a match.
    ///
    /// Games use this as a mid-frame trigger — the classic use is splitting the
    /// screen so a status bar stays put while the rest scrolls.
    fn check_lyc(&mut self, interrupts: &mut InterruptState) {
        let matched = self.ly == self.lyc;
        if matched {
            self.stat |= 0x04;
            if self.stat & 0x40 != 0 {
                interrupts.request(Interrupt::Stat);
            }
        } else {
            self.stat &= !0x04;
        }
    }

    // --- Register access ---

    pub fn read(&self, address: u16) -> u8 {
        match address {
            0xFF40 => self.lcdc,
            // Bit 7 is unimplemented and reads as 1; bits 0-2 are the live mode and
            // LYC flag rather than what was written.
            0xFF41 => self.stat | 0x80 | self.mode as u8,
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly,
            0xFF45 => self.lyc,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0xFF40 => {
                let was_on = self.lcd_enabled();
                self.lcdc = value;
                if was_on && !self.lcd_enabled() {
                    // Switching the LCD off resets the state machine: LY goes to 0
                    // and the PPU parks in mode 0. Games do this before bulk VRAM
                    // updates, since with the LCD off VRAM is always accessible.
                    self.ly = 0;
                    self.line_cycles = 0;
                    self.window_line = 0;
                    self.mode = Mode::HBlank;
                } else if !was_on && self.lcd_enabled() {
                    self.line_cycles = 0;
                    self.mode = Mode::OamScan;
                }
            }
            // Only bits 3-6 are writable; the mode and LYC flag are read-only.
            0xFF41 => self.stat = (self.stat & 0x87) | (value & 0x78),
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            // LY is read-only. Writes are ignored, not stored.
            0xFF44 => {}
            0xFF45 => self.lyc = value,
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => self.wy = value,
            0xFF4B => self.wx = value,
            _ => {}
        }
    }

    /// VRAM is inaccessible to the CPU during mode 3, when the PPU is reading it.
    pub fn read_vram(&self, address: u16) -> u8 {
        if self.lcd_enabled() && self.mode == Mode::Drawing {
            return 0xFF;
        }
        self.vram[(address & 0x1FFF) as usize]
    }

    pub fn write_vram(&mut self, address: u16, value: u8) {
        if self.lcd_enabled() && self.mode == Mode::Drawing {
            return;
        }
        self.vram[(address & 0x1FFF) as usize] = value;
    }

    /// OAM is inaccessible during both the sprite scan and drawing.
    pub fn read_oam(&self, address: u16) -> u8 {
        if self.lcd_enabled() && matches!(self.mode, Mode::OamScan | Mode::Drawing) {
            return 0xFF;
        }
        self.oam[(address & 0xFF) as usize]
    }

    pub fn write_oam(&mut self, address: u16, value: u8) {
        if self.lcd_enabled() && matches!(self.mode, Mode::OamScan | Mode::Drawing) {
            return;
        }
        self.oam[(address & 0xFF) as usize] = value;
    }

    /// Writes OAM regardless of mode, for DMA transfers.
    ///
    /// OAM DMA bypasses the normal restrictions because it is the DMA controller
    /// writing, not the CPU.
    pub fn write_oam_dma(&mut self, offset: u8, value: u8) {
        self.oam[offset as usize] = value;
    }

    pub fn framebuffer(&self) -> &[Shade; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &self.framebuffer
    }

    /// The current mode. Used by tests now; the frontend will want it too.
    #[cfg(test)]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    #[cfg(test)]
    pub fn ly(&self) -> u8 {
        self.ly
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
