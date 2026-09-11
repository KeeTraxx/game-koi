//! PPU tests.

use super::*;

fn interrupts() -> InterruptState {
    InterruptState {
        enabled: 0xFF,
        ..Default::default()
    }
}

/// Advances the PPU by a number of T-cycles.
fn tick_t(ppu: &mut Ppu, ints: &mut InterruptState, t_cycles: u32) {
    for _ in 0..t_cycles / 4 {
        ppu.tick(ints);
    }
}

/// Writes a tile into VRAM at the given tile index, from 8 rows of colour indices.
fn write_tile(ppu: &mut Ppu, tile: usize, rows: [[u8; 8]; 8]) {
    for (y, row) in rows.iter().enumerate() {
        let mut low = 0u8;
        let mut high = 0u8;
        for (x, &colour) in row.iter().enumerate() {
            let bit = 7 - x;
            low |= (colour & 1) << bit;
            high |= ((colour >> 1) & 1) << bit;
        }
        ppu.vram[tile * 16 + y * 2] = low;
        ppu.vram[tile * 16 + y * 2 + 1] = high;
    }
}

#[test]
fn modes_cycle_in_hardware_order() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    assert_eq!(ppu.mode(), Mode::OamScan, "a line starts scanning OAM");

    tick_t(&mut ppu, &mut ints, 80);
    assert_eq!(ppu.mode(), Mode::Drawing);

    tick_t(&mut ppu, &mut ints, 172);
    assert_eq!(ppu.mode(), Mode::HBlank);

    // The rest of the 456-cycle line is HBlank, then the next line starts.
    tick_t(&mut ppu, &mut ints, 456 - 80 - 172);
    assert_eq!(ppu.mode(), Mode::OamScan);
    assert_eq!(ppu.ly(), 1);
}

#[test]
fn a_frame_is_154_lines_and_70224_cycles() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    // Run exactly one frame.
    tick_t(&mut ppu, &mut ints, 456 * 154);
    assert_eq!(ppu.ly(), 0, "wrapped back to the top");
    assert!(ppu.frame_ready);
}

#[test]
fn vblank_starts_at_line_144_and_raises_an_interrupt() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    tick_t(&mut ppu, &mut ints, 456 * 144);
    assert_eq!(ppu.ly(), 144);
    assert_eq!(ppu.mode(), Mode::VBlank);
    assert_eq!(
        ints.pending(),
        Some(Interrupt::VBlank),
        "VBlank is the frame heartbeat"
    );
    assert!(ppu.frame_ready);
}

#[test]
fn vblank_lasts_ten_lines() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    tick_t(&mut ppu, &mut ints, 456 * 144);
    assert_eq!(ppu.mode(), Mode::VBlank);

    // Still in VBlank on line 153, the last one.
    tick_t(&mut ppu, &mut ints, 456 * 9);
    assert_eq!(ppu.ly(), 153);
    assert_eq!(ppu.mode(), Mode::VBlank);

    tick_t(&mut ppu, &mut ints, 456);
    assert_eq!(ppu.ly(), 0, "back to the top");
    assert_eq!(ppu.mode(), Mode::OamScan);
}

#[test]
fn stat_reports_the_live_mode() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    assert_eq!(ppu.read(0xFF41) & 0x03, Mode::OamScan as u8);
    tick_t(&mut ppu, &mut ints, 80);
    assert_eq!(ppu.read(0xFF41) & 0x03, Mode::Drawing as u8);
    // Bit 7 is unimplemented and reads as 1.
    assert_eq!(ppu.read(0xFF41) & 0x80, 0x80);
}

#[test]
fn stat_mode_bits_are_read_only() {
    let mut ppu = Ppu::new();
    // Try to write a mode; only bits 3-6 should stick.
    ppu.write(0xFF41, 0xFF);
    assert_eq!(
        ppu.read(0xFF41) & 0x03,
        Mode::OamScan as u8,
        "mode unchanged"
    );
    assert_eq!(
        ppu.read(0xFF41) & 0x78,
        0x78,
        "interrupt enables were written"
    );
}

#[test]
fn ly_is_read_only() {
    let mut ppu = Ppu::new();
    ppu.write(0xFF44, 0x50);
    assert_eq!(ppu.ly(), 0, "writes to LY are ignored");
}

#[test]
fn lyc_match_sets_the_flag_and_can_interrupt() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF45, 5); // LYC = 5
    ppu.write(0xFF41, 0x40); // enable the LYC interrupt

    tick_t(&mut ppu, &mut ints, 456 * 5);
    assert_eq!(ppu.ly(), 5);
    assert_eq!(ppu.read(0xFF41) & 0x04, 0x04, "LYC=LY flag set");
    assert_eq!(ints.pending(), Some(Interrupt::Stat));

    // Moving off the matching line clears the flag again.
    ints.acknowledge(Interrupt::Stat);
    tick_t(&mut ppu, &mut ints, 456);
    assert_eq!(ppu.read(0xFF41) & 0x04, 0, "flag cleared");
}

#[test]
fn mode_stat_interrupts_fire_only_when_enabled() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    // Nothing enabled: reaching HBlank raises nothing.
    tick_t(&mut ppu, &mut ints, 80 + 172);
    assert_eq!(ppu.mode(), Mode::HBlank);
    assert_eq!(ints.pending(), None);

    // Enable the HBlank STAT interrupt and run to the next one.
    ppu.write(0xFF41, 0x08);
    tick_t(&mut ppu, &mut ints, 456);
    assert_eq!(ints.pending(), Some(Interrupt::Stat));
}

#[test]
fn vram_is_locked_during_drawing() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write_vram(0x8000, 0x42);
    assert_eq!(ppu.read_vram(0x8000), 0x42);

    tick_t(&mut ppu, &mut ints, 80);
    assert_eq!(ppu.mode(), Mode::Drawing);
    assert_eq!(ppu.read_vram(0x8000), 0xFF, "VRAM reads as 0xFF in mode 3");

    // Writes are dropped too, not queued.
    ppu.write_vram(0x8000, 0x99);
    tick_t(&mut ppu, &mut ints, 172);
    assert_eq!(ppu.read_vram(0x8000), 0x42, "the blocked write was lost");
}

#[test]
fn oam_is_locked_during_scan_and_drawing() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    // Mode 2 from the start: OAM already locked.
    assert_eq!(ppu.mode(), Mode::OamScan);
    assert_eq!(ppu.read_oam(0xFE00), 0xFF);

    // In HBlank it is free again.
    tick_t(&mut ppu, &mut ints, 80 + 172);
    assert_eq!(ppu.mode(), Mode::HBlank);
    ppu.write_oam(0xFE00, 0x42);
    assert_eq!(ppu.read_oam(0xFE00), 0x42);
}

#[test]
fn dma_writes_bypass_oam_locking() {
    let mut ppu = Ppu::new();
    // Mode 2 locks OAM against the CPU...
    assert_eq!(ppu.mode(), Mode::OamScan);
    ppu.write_oam(0xFE00, 0x11);
    assert_eq!(ppu.oam[0], 0x00, "CPU write blocked");

    // ...but the DMA controller writes regardless.
    ppu.write_oam_dma(0, 0x22);
    assert_eq!(ppu.oam[0], 0x22);
}

#[test]
fn disabling_the_lcd_resets_the_state_machine() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    tick_t(&mut ppu, &mut ints, 456 * 10);
    assert_eq!(ppu.ly(), 10);

    ppu.write(0xFF40, 0x00); // LCD off
    assert_eq!(ppu.ly(), 0, "LY resets");
    assert_eq!(ppu.mode(), Mode::HBlank);

    // With the LCD off, VRAM is always accessible.
    ppu.write_vram(0x8000, 0x77);
    assert_eq!(ppu.read_vram(0x8000), 0x77);

    // And the PPU does not advance.
    tick_t(&mut ppu, &mut ints, 456 * 5);
    assert_eq!(ppu.ly(), 0);
}

#[test]
fn palette_maps_indices_to_shades() {
    // BGP = 0b11_10_01_00: identity mapping.
    assert_eq!(Ppu::shade(0xE4, 0), 0);
    assert_eq!(Ppu::shade(0xE4, 1), 1);
    assert_eq!(Ppu::shade(0xE4, 2), 2);
    assert_eq!(Ppu::shade(0xE4, 3), 3);

    // An inverted palette turns index 0 into the darkest shade.
    assert_eq!(Ppu::shade(0x1B, 0), 3);
    assert_eq!(Ppu::shade(0x1B, 3), 0);
}

#[test]
fn tile_rows_interleave_two_bitplanes() {
    let mut ppu = Ppu::new();
    // Low byte 0b10101010, high byte 0b11001100 gives colours from combining bits.
    ppu.vram[0] = 0b1010_1010;
    ppu.vram[1] = 0b1100_1100;

    let row = ppu.tile_row(0, 0);
    // Pixel 0 takes bit 7 of each byte: low 1, high 1 -> colour 3.
    // Pixel 1 takes bit 6: low 0, high 1 -> 2. Pixel 2 takes bit 5: low 1, high 0 -> 1.
    assert_eq!(row, [3, 2, 1, 0, 3, 2, 1, 0]);
}

#[test]
fn signed_tile_addressing_reaches_backwards() {
    let mut ppu = Ppu::new();

    // TILE_SEL set: unsigned, tile 0 at VRAM offset 0.
    ppu.write(0xFF40, 0x90);
    assert_eq!(ppu.tile_address(0), 0x0000);
    assert_eq!(ppu.tile_address(1), 0x0010);

    // TILE_SEL clear: signed, tile 0 at 0x9000 (offset 0x1000).
    ppu.write(0xFF40, 0x80);
    assert_eq!(ppu.tile_address(0), 0x1000);
    // Tile 0xFF is -1, one tile *below* the base.
    assert_eq!(ppu.tile_address(0xFF), 0x0FF0);
    // Tile 0x80 is -128, reaching down to 0x8800.
    assert_eq!(ppu.tile_address(0x80), 0x0800);
}

#[test]
fn renders_a_solid_background_tile() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    // LCD on, BG on, unsigned tile addressing, BG map at 0x9800.
    ppu.write(0xFF40, 0x91);
    ppu.write(0xFF47, 0xE4); // identity palette

    // Tile 1 is solid colour 3; point the whole map at it.
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1;
    }

    // Run one full frame so every line is drawn.
    tick_t(&mut ppu, &mut ints, 456 * 154);

    let fb = ppu.framebuffer();
    assert!(
        fb.iter().all(|&shade| shade == 3),
        "whole screen is shade 3"
    );
}

#[test]
fn scroll_registers_move_the_background() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x91);
    ppu.write(0xFF47, 0xE4);

    // Tile 0 blank, tile 1 solid. Put tile 1 only in map column 0.
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    for row in 0..32 {
        ppu.vram[0x1800 + row * 32] = 1;
    }

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[0], 3, "leftmost 8 pixels are the solid tile");
    assert_eq!(fb[8], 0, "then blank");

    // Scrolling right by 8 brings the next (blank) tile to the left edge.
    let mut ppu2 = Ppu::new();
    let mut ints2 = interrupts();
    ppu2.write(0xFF40, 0x91);
    ppu2.write(0xFF47, 0xE4);
    write_tile(&mut ppu2, 1, [[3; 8]; 8]);
    for row in 0..32 {
        ppu2.vram[0x1800 + row * 32] = 1;
    }
    ppu2.write(0xFF43, 8); // SCX = 8
    tick_t(&mut ppu2, &mut ints2, 456 * 154);
    assert_eq!(ppu2.framebuffer()[0], 0, "scrolled past the solid column");
}

#[test]
fn bg_disabled_blanks_the_screen() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    // LCD on but BG_EN clear.
    ppu.write(0xFF40, 0x90);
    ppu.write(0xFF47, 0xE4);
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1;
    }

    tick_t(&mut ppu, &mut ints, 456 * 154);
    assert!(
        ppu.framebuffer().iter().all(|&s| s == 0),
        "BG_EN clear blanks the background"
    );
}

#[test]
fn renders_a_sprite_over_the_background() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    // LCD on, sprites on, BG on.
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4); // OBP0 identity

    // Tile 1: solid colour 1, used as a sprite.
    write_tile(&mut ppu, 1, [[1; 8]; 8]);

    // Sprite at screen (0, 0): OAM Y and X are offset by 16 and 8.
    ppu.oam[0] = 16;
    ppu.oam[1] = 8;
    ppu.oam[2] = 1;
    ppu.oam[3] = 0;

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[0], 1, "sprite drawn at the top-left");
    assert_eq!(fb[8], 0, "and only 8 pixels wide");
}

#[test]
fn sprite_colour_zero_is_transparent() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);

    // Background solid shade 3.
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1;
    }
    // Sprite tile 2 is entirely colour 0 — fully transparent.
    write_tile(&mut ppu, 2, [[0; 8]; 8]);
    ppu.oam[0] = 16;
    ppu.oam[1] = 8;
    ppu.oam[2] = 2;
    ppu.oam[3] = 0;

    tick_t(&mut ppu, &mut ints, 456 * 154);
    assert_eq!(
        ppu.framebuffer()[0],
        3,
        "transparent sprite pixels leave the background alone"
    );
}

#[test]
fn sprite_priority_flag_puts_it_behind_the_background() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);

    // Background colour 1 (non-zero, so it wins against a behind-BG sprite).
    write_tile(&mut ppu, 1, [[1; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1;
    }
    write_tile(&mut ppu, 2, [[3; 8]; 8]);
    ppu.oam[0] = 16;
    ppu.oam[1] = 8;
    ppu.oam[2] = 2;
    ppu.oam[3] = 0x80; // behind background

    tick_t(&mut ppu, &mut ints, 456 * 154);
    assert_eq!(ppu.framebuffer()[0], 1, "background wins");
}

#[test]
fn sprite_flips_mirror_the_tile() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);

    // A tile with colour 2 only in its leftmost column.
    let mut rows = [[0u8; 8]; 8];
    for row in rows.iter_mut() {
        row[0] = 2;
    }
    write_tile(&mut ppu, 1, rows);

    ppu.oam[0] = 16;
    ppu.oam[1] = 8;
    ppu.oam[2] = 1;
    ppu.oam[3] = 0x20; // flip X

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[0], 0, "left column is now empty");
    assert_eq!(fb[7], 2, "the mark moved to the right edge");
}

#[test]
fn only_ten_sprites_are_drawn_per_line() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);
    write_tile(&mut ppu, 1, [[1; 8]; 8]);

    // 12 sprites on line 0, each 8 pixels further right.
    for index in 0..12 {
        ppu.oam[index * 4] = 16;
        ppu.oam[index * 4 + 1] = 8 + (index as u8 * 8);
        ppu.oam[index * 4 + 2] = 1;
        ppu.oam[index * 4 + 3] = 0;
    }

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[0], 1, "the first sprite drew");
    assert_eq!(fb[9 * 8], 1, "the tenth sprite drew");
    assert_eq!(fb[10 * 8], 0, "the eleventh was dropped");
}

#[test]
fn tall_sprites_use_two_stacked_tiles() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    // OBJ_SIZE set: 8x16 sprites.
    ppu.write(0xFF40, 0x97);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);

    // Tile 2 is the top half, tile 3 the bottom.
    write_tile(&mut ppu, 2, [[1; 8]; 8]);
    write_tile(&mut ppu, 3, [[3; 8]; 8]);

    // The tile number's low bit is ignored, so 3 selects the pair (2, 3).
    ppu.oam[0] = 16;
    ppu.oam[1] = 8;
    ppu.oam[2] = 3;
    ppu.oam[3] = 0;

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[0], 1, "top half from the even tile");
    assert_eq!(fb[8 * SCREEN_WIDTH], 3, "bottom half from the odd one");
}

#[test]
fn window_draws_over_the_background() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    // LCD on, window on, BG on, window map at 0x9800 (WIN_MAP clear).
    ppu.write(0xFF40, 0xB1);
    ppu.write(0xFF47, 0xE4);

    // Background tile 1 (shade 3) everywhere, window tile 2 (shade 1).
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    write_tile(&mut ppu, 2, [[1; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 2; // shared map: window and BG both use it
    }

    ppu.write(0xFF4A, 0); // WY = 0
    ppu.write(0xFF4B, 7); // WX = 7, so the window starts at x = 0

    tick_t(&mut ppu, &mut ints, 456 * 154);
    // The window covers the screen, so everything is the window's tile.
    assert_eq!(ppu.framebuffer()[0], 1);
}

#[test]
fn window_position_offsets_by_seven() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    // LCD on, window on, BG on. WIN_MAP set puts the window map at 0x9C00, so the
    // window and background read *different* maps and can be told apart.
    ppu.write(0xFF40, 0xF1);
    ppu.write(0xFF47, 0xE4);

    write_tile(&mut ppu, 1, [[3; 8]; 8]); // background tile
    write_tile(&mut ppu, 2, [[1; 8]; 8]); // window tile
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1; // BG map -> tile 1
        ppu.vram[0x1C00 + offset] = 2; // window map -> tile 2
    }

    ppu.write(0xFF4A, 0); // WY = 0
    ppu.write(0xFF4B, 7 + 80); // WX puts the window's left edge at x = 80

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[0], 3, "background left of the window");
    assert_eq!(fb[79], 3, "still background one pixel before");
    assert_eq!(fb[80], 1, "window starts exactly at WX-7");
    assert_eq!(fb[159], 1, "and continues to the right edge");
}

#[test]
fn window_at_wx_seven_covers_the_whole_line() {
    // WX = 7 is the leftmost position; anything less is an edge case games avoid.
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0xF1);
    ppu.write(0xFF47, 0xE4);
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    write_tile(&mut ppu, 2, [[1; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1;
        ppu.vram[0x1C00 + offset] = 2;
    }
    ppu.write(0xFF4A, 0);
    ppu.write(0xFF4B, 7);

    tick_t(&mut ppu, &mut ints, 456 * 154);
    assert_eq!(ppu.framebuffer()[0], 1, "window covers from x = 0");
}

#[test]
fn window_starts_below_wy() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0xF1);
    ppu.write(0xFF47, 0xE4);
    write_tile(&mut ppu, 1, [[3; 8]; 8]);
    write_tile(&mut ppu, 2, [[1; 8]; 8]);
    for offset in 0..32 * 32 {
        ppu.vram[0x1800 + offset] = 1;
        ppu.vram[0x1C00 + offset] = 2;
    }
    ppu.write(0xFF4A, 64); // WY = 64
    ppu.write(0xFF4B, 7);

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[63 * SCREEN_WIDTH], 3, "background above WY");
    assert_eq!(fb[64 * SCREEN_WIDTH], 1, "window from WY down");
}

#[test]
fn overlapping_sprites_resolve_by_x_then_oam_order() {
    // DMG priority: the sprite with the smaller X wins. Two sprites overlap at the
    // same pixel, and the one further left must be the visible one.
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);

    write_tile(&mut ppu, 1, [[1; 8]; 8]); // shade 1
    write_tile(&mut ppu, 2, [[3; 8]; 8]); // shade 3

    // Sprite 0 is at x = 4 with the *higher* shade; sprite 1 is at x = 0.
    // Smaller X wins, so the pixel at x = 4 must show sprite 1's shade.
    ppu.oam[0] = 16;
    ppu.oam[1] = 8 + 4;
    ppu.oam[2] = 2;
    ppu.oam[3] = 0;
    ppu.oam[4] = 16;
    ppu.oam[5] = 8;
    ppu.oam[6] = 1;
    ppu.oam[7] = 0;

    tick_t(&mut ppu, &mut ints, 456 * 154);
    let fb = ppu.framebuffer();
    assert_eq!(fb[4], 1, "the leftmost sprite wins the overlap");
    assert_eq!(fb[8], 3, "past its right edge, the other shows");
}

#[test]
fn equal_x_sprites_resolve_by_oam_order() {
    // With identical X, the earlier OAM entry wins.
    let mut ppu = Ppu::new();
    let mut ints = interrupts();
    ppu.write(0xFF40, 0x93);
    ppu.write(0xFF47, 0xE4);
    ppu.write(0xFF48, 0xE4);

    write_tile(&mut ppu, 1, [[1; 8]; 8]);
    write_tile(&mut ppu, 2, [[3; 8]; 8]);

    ppu.oam[0] = 16;
    ppu.oam[1] = 8;
    ppu.oam[2] = 1; // shade 1, OAM index 0
    ppu.oam[3] = 0;
    ppu.oam[4] = 16;
    ppu.oam[5] = 8;
    ppu.oam[6] = 2; // shade 3, OAM index 1
    ppu.oam[7] = 0;

    tick_t(&mut ppu, &mut ints, 456 * 154);
    assert_eq!(ppu.framebuffer()[0], 1, "the earlier OAM entry wins");
}

#[test]
fn frame_ready_signals_once_per_frame() {
    let mut ppu = Ppu::new();
    let mut ints = interrupts();

    tick_t(&mut ppu, &mut ints, 456 * 143);
    assert!(!ppu.frame_ready, "not finished yet");

    tick_t(&mut ppu, &mut ints, 456);
    assert!(ppu.frame_ready, "frame completed at line 144");
}

/// A whole-pipeline check: a CPU program sets up VRAM and turns the LCD on, and the
/// PPU renders it. Everything below the frontend is exercised at once.
///
/// The LCD must be switched off before writing VRAM in bulk — with it on, mode 3
/// blocks CPU access and most of the writes are silently dropped. Real ROMs do
/// exactly this, and getting it wrong produces a blank screen rather than an error.
#[test]
fn cpu_program_draws_through_the_ppu() {
    use crate::bus::Bus;
    use crate::cpu::Cpu;

    /// Minimal bus: ROM low, PPU-backed VRAM, RAM elsewhere.
    struct DrawBus {
        rom: Vec<u8>,
        memory: Vec<u8>,
        ppu: Ppu,
        interrupts: InterruptState,
    }

    impl Bus for DrawBus {
        fn read(&mut self, address: u16) -> u8 {
            match address {
                0x0000..=0x7FFF => self.rom[address as usize],
                0x8000..=0x9FFF => self.ppu.read_vram(address),
                0xFF40..=0xFF4B => self.ppu.read(address),
                _ => self.memory[address as usize],
            }
        }
        fn write(&mut self, address: u16, value: u8) {
            match address {
                0x0000..=0x7FFF => {}
                0x8000..=0x9FFF => self.ppu.write_vram(address, value),
                0xFF40..=0xFF4B => self.ppu.write(address, value),
                _ => self.memory[address as usize] = value,
            }
        }
        fn tick(&mut self) {
            self.ppu.tick(&mut self.interrupts);
        }
        fn pending_interrupt(&self) -> Option<Interrupt> {
            self.interrupts.pending()
        }
        fn acknowledge_interrupt(&mut self, interrupt: Interrupt) {
            self.interrupts.acknowledge(interrupt);
        }
    }

    let mut rom = vec![0u8; 0x8000];
    // A solid colour-3 tile stored in ROM, to be copied into VRAM as tile 1.
    for byte in rom.iter_mut().skip(0x0600).take(16) {
        *byte = 0xFF;
    }

    let program: &[u8] = &[
        0x3E, 0x00, 0xE0, 0x40, // LD A,0; LDH (LCDC),A  -- LCD off first
        0x21, 0x10, 0x80, // LD HL,0x8010   (tile 1)
        0x11, 0x00, 0x06, // LD DE,0x0600
        0x06, 0x10, // LD B,16
        0x1A, 0x22, 0x13, 0x05, 0x20, 0xFA, // copy loop
        0x21, 0x00, 0x98, // LD HL,0x9800   (tile map)
        0x3E, 0x01, // LD A,1
        0x06, 0xFF, // LD B,255
        0x22, 0x05, 0x20, 0xFC, // fill loop: LD (HL+),A; DEC B; JR NZ
        0x3E, 0xE4, 0xE0, 0x47, // BGP = identity
        0x3E, 0x91, 0xE0, 0x40, // LCDC = LCD on + BG on
        0x18, 0xFE, // spin
    ];
    rom[0x0100..0x0100 + program.len()].copy_from_slice(program);

    let mut bus = DrawBus {
        rom,
        memory: vec![0; 0x1_0000],
        ppu: Ppu::new(),
        interrupts: InterruptState::default(),
    };
    let mut cpu = Cpu::new();

    // Long enough to finish setup and render a couple of frames.
    for _ in 0..200_000 {
        cpu.step(&mut bus);
    }

    let fb = bus.ppu.framebuffer();
    assert_eq!(fb[0], 3, "top-left shows the solid tile");
    assert!(
        fb.iter().take(SCREEN_WIDTH).all(|&s| s == 3),
        "the first scanline is fully covered"
    );
}
