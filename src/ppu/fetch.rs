//! Turning VRAM contents into pixels.
//!
//! # How a tile is stored
//!
//! Everything on screen is built from 8x8 tiles. A tile is 16 bytes: two bytes per
//! row, and the two bits of a pixel are split across them — the *low* bit of the
//! colour comes from the first byte, the *high* bit from the second, both at the
//! same bit position. So a pixel is assembled by taking one bit from each byte
//! rather than reading a nibble.
//!
//! Bit 7 is the leftmost pixel, so the byte reads in the same order as the pixels.
//!
//! # How a colour becomes a shade
//!
//! A pixel's two bits are an *index*, not a colour. The palette register packs four
//! 2-bit shades, index 0 in the low bits. Games animate palettes to fade the screen
//! without touching a single tile.
//!
//! Index 0 is special for sprites: it means transparent, and is never drawn.

use super::{Ppu, SCREEN_HEIGHT, SCREEN_WIDTH, Shade};

/// A sprite as stored in OAM: 4 bytes.
struct Sprite {
    /// Screen Y position, offset by 16 so sprites can scroll off the top.
    y: i16,
    /// Screen X position, offset by 8 so sprites can scroll off the left.
    x: i16,
    tile: u8,
    flags: u8,
    /// Position in OAM, which breaks priority ties.
    index: usize,
}

impl Sprite {
    /// Drawn behind background colours 1-3 when set.
    fn behind_background(&self) -> bool {
        self.flags & 0x80 != 0
    }
    fn flip_y(&self) -> bool {
        self.flags & 0x40 != 0
    }
    fn flip_x(&self) -> bool {
        self.flags & 0x20 != 0
    }
    /// Which of the two sprite palettes to use.
    fn uses_obp1(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

impl Ppu {
    /// Applies a palette register to a 2-bit colour index.
    pub(super) fn shade(palette: u8, colour: u8) -> Shade {
        (palette >> (colour * 2)) & 0x03
    }

    /// Reads one row of a tile and returns its eight colour indices, left to right.
    ///
    /// `tile_address` is the tile's base offset in VRAM; `row` is 0-7.
    pub(super) fn tile_row(&self, tile_address: u16, row: u16) -> [u8; 8] {
        let base = (tile_address + row * 2) as usize;
        let low = self.vram[base & 0x1FFF];
        let high = self.vram[(base + 1) & 0x1FFF];

        let mut pixels = [0u8; 8];
        for (offset, pixel) in pixels.iter_mut().enumerate() {
            // Bit 7 is the leftmost pixel.
            let bit = 7 - offset;
            *pixel = ((low >> bit) & 1) | (((high >> bit) & 1) << 1);
        }
        pixels
    }

    /// Resolves a tile number from a map into a VRAM offset.
    ///
    /// Two addressing modes, selected by LCDC's TILE_SEL bit. The unsigned mode is
    /// straightforward: tiles start at 0x8000. The signed mode treats the tile
    /// number as an `i8` relative to 0x9000, so tile 0 is in the middle of VRAM and
    /// negative numbers reach back to 0x8800. That exists so two tile sets can share
    /// the region, and it is a classic source of "everything is garbage" bugs.
    pub(super) fn tile_address(&self, tile: u8) -> u16 {
        if self.signed_tile_addressing() {
            // 0x9000 is VRAM offset 0x1000.
            (0x1000_i32 + (tile as i8 as i32) * 16) as u16
        } else {
            tile as u16 * 16
        }
    }

    /// Renders the current scanline into the framebuffer.
    pub(super) fn draw_line(&mut self) {
        let line = self.ly as usize;
        if line >= SCREEN_HEIGHT {
            return;
        }

        // Colour indices for this line, kept alongside the shades because sprite
        // priority is decided against the background *index*, not its shade. A
        // background showing index 0 is "behind" even if the palette makes it dark.
        let mut bg_colours = [0u8; SCREEN_WIDTH];

        if self.bg_enabled() {
            self.draw_background_line(&mut bg_colours);
        } else {
            // With BG_EN clear the background is blank white, and the window goes
            // with it on a DMG.
            let start = line * SCREEN_WIDTH;
            self.framebuffer[start..start + SCREEN_WIDTH].fill(0);
        }

        if self.sprites_enabled() {
            self.draw_sprite_line(&bg_colours);
        }
    }

    /// Draws the background and, where it is visible, the window.
    fn draw_background_line(&mut self, bg_colours: &mut [u8; SCREEN_WIDTH]) {
        let line = self.ly;
        // The window starts on the first line where WY has been reached, and its own
        // counter advances only on lines where it actually appears.
        let window_visible = self.window_enabled() && line >= self.wy;
        let mut window_drawn = false;

        #[allow(clippy::needless_range_loop)] // x is also arithmetic, not just an index
        for x in 0..SCREEN_WIDTH {
            // WX is stored offset by 7, so WX=7 puts the window at the left edge.
            let window_here = window_visible && (x as i16) >= (self.wx as i16 - 7);

            let colour = if window_here {
                window_drawn = true;
                let window_x = (x as i16 - (self.wx as i16 - 7)) as u16;
                let window_y = self.window_line as u16;
                self.map_pixel(self.window_map_base(), window_x, window_y)
            } else {
                // The background is a 256x256 plane that wraps, scrolled by SCX/SCY.
                let bg_x = (x as u16 + self.scx as u16) & 0xFF;
                let bg_y = (line as u16 + self.scy as u16) & 0xFF;
                self.map_pixel(self.bg_map_base(), bg_x, bg_y)
            };

            bg_colours[x] = colour;
            self.framebuffer[line as usize * SCREEN_WIDTH + x] = Self::shade(self.bgp, colour);
        }

        // The window's line counter tracks lines *it* was drawn on, not screen lines,
        // so a window that starts partway down still begins at its own row 0.
        if window_drawn {
            self.window_line += 1;
        }
    }

    /// Looks up one pixel of a tile map. `x` and `y` are in map space (0-255).
    fn map_pixel(&self, map_base: u16, x: u16, y: u16) -> u8 {
        // The map is 32x32 tile numbers, one byte each.
        let map_offset = map_base + (y / 8) * 32 + (x / 8);
        let tile = self.vram[(map_offset & 0x1FFF) as usize];
        let address = self.tile_address(tile);
        self.tile_row(address, y % 8)[(x % 8) as usize]
    }

    /// Draws sprites intersecting this scanline.
    fn draw_sprite_line(&mut self, bg_colours: &[u8; SCREEN_WIDTH]) {
        let line = self.ly as i16;
        let height: i16 = if self.tall_sprites() { 16 } else { 8 };

        // Find sprites on this line. Hardware scans OAM in order and stops at 10 —
        // a real limit games work within, and why sprites sometimes flicker.
        let mut visible: Vec<Sprite> = Vec::new();
        for index in 0..40 {
            let base = index * 4;
            let sprite = Sprite {
                y: self.oam[base] as i16 - 16,
                x: self.oam[base + 1] as i16 - 8,
                tile: self.oam[base + 2],
                flags: self.oam[base + 3],
                index,
            };
            if line >= sprite.y && line < sprite.y + height {
                visible.push(sprite);
                if visible.len() == 10 {
                    break;
                }
            }
        }

        // Priority on a DMG: smaller X wins, and OAM order breaks ties. Drawing back
        // to front means the winner is drawn last and covers the others.
        visible.sort_by_key(|sprite| (-sprite.x, -(sprite.index as i16)));

        for sprite in &visible {
            let mut row = line - sprite.y;
            if sprite.flip_y() {
                row = height - 1 - row;
            }

            // A tall sprite is two stacked tiles, and the hardware ignores bit 0 of
            // the tile number so the pair is always aligned.
            let tile = if self.tall_sprites() {
                (sprite.tile & 0xFE) + (row >= 8) as u8
            } else {
                sprite.tile
            };

            // Sprites always use unsigned addressing from 0x8000, regardless of the
            // TILE_SEL bit that governs background tiles.
            let pixels = self.tile_row(tile as u16 * 16, (row % 8) as u16);
            let palette = if sprite.uses_obp1() {
                self.obp1
            } else {
                self.obp0
            };

            for offset in 0..8 {
                let x = sprite.x + offset;
                if x < 0 || x >= SCREEN_WIDTH as i16 {
                    continue;
                }

                let index = if sprite.flip_x() { 7 - offset } else { offset };
                let colour = pixels[index as usize];

                // Colour 0 is transparent for sprites — not a shade at all.
                if colour == 0 {
                    continue;
                }
                // A background-priority sprite hides behind non-zero background.
                if sprite.behind_background() && bg_colours[x as usize] != 0 {
                    continue;
                }

                self.framebuffer[self.ly as usize * SCREEN_WIDTH + x as usize] =
                    Self::shade(palette, colour);
            }
        }
    }
}
