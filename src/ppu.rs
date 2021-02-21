#![allow(non_snake_case)]
use anyhow::Result;
use sdl2::pixels::Color;

use crate::display::*;

#[derive(Default)]
struct PPUInterrupts {
    vblank: bool,
    stat: bool,
}

#[derive(Default)]
struct PPURegisters {
    LCDC: u8,
    LY: u8,
    LYC: u8,
    STAT: u8,
    SCY: u8,
    SCX: u8,
    WY: u8,
    WX: u8,
    WC: u8,
    BGP: u8,
    OBP0: u8,
    OBP1: u8,
    interrupts: PPUInterrupts,
}

impl PPURegisters {
    pub fn new() -> Self {
        PPURegisters {
            STAT: 0x80,
            ..Default::default()
        }
    }
}

pub struct PPU {
    pub memory: [u8; 0x2000],
    pub oam: [u8; 0xA0],
    viewport: [[Color; W_WIDTH]; W_HEIGHT],
    registers: PPURegisters,
    display: Display,
    enable_display_events: bool,
    block_stat_irqs: bool,
    cycles: u64,
}

impl PPU {
    pub fn new() -> Result<Self> {
        Ok(PPU {
            memory: [0; 0x2000],
            oam: [0; 0xA0],
            viewport: [[Color::WHITE; W_WIDTH]; W_HEIGHT],
            registers: PPURegisters::new(),
            display: Display::new()?,
            enable_display_events: false,
            block_stat_irqs: false,
            cycles: 0,
        })
    }

    pub fn read_byte(&self, addr: u16) -> u8 {
        match addr {
            0x8000..=0x9FFF => self.memory[addr as usize - 0x8000],
            0xFE00..=0xFE9F => self.oam[addr as usize - 0xFE00],
            0xFF40 => self.registers.LCDC,
            0xFF41 => self.registers.STAT,
            0xFF42 => self.registers.SCY,
            0xFF43 => self.registers.SCX,
            0xFF44 => self.registers.LY,
            0xFF45 => self.registers.LYC,
            0xFF47 => self.registers.BGP,
            0xFF48 => self.registers.OBP0,
            0xFF49 => self.registers.OBP1,
            0xFF4A => self.registers.WY,
            0xFF4B => self.registers.WX,
            _ => panic!("Invalid PPU Register read: {:04x}", addr),
        }
    }

    pub fn write_byte(&mut self, addr: u16, val: u8) {
        match addr {
            0x8000..=0x9FFF => self.memory[addr as usize - 0x8000] = val,
            0xFE00..=0xFE9F => self.oam[addr as usize - 0xFE00] = val,
            0xFF40 => self.registers.LCDC = val,
            0xFF41 => self.registers.STAT |= val & !7,
            0xFF42 => self.registers.SCY = val,
            0xFF43 => self.registers.SCX = val,
            0xFF45 => self.registers.LYC = val,
            0xFF47 => self.registers.BGP = val,
            0xFF48 => self.registers.OBP0 = val,
            0xFF49 => self.registers.OBP1 = val,
            0xFF4A => self.registers.WY = val,
            0xFF4B => self.registers.WX = val,
            _ => panic!("Invalid PPU Register write: {:04x}", addr),
        }
    }

    pub fn poll_display_event(&mut self) -> DisplayEvent {
        if self.enable_display_events {
            self.enable_display_events = false;
            self.display.poll_event()
        } else {
            DisplayEvent::None
        }
    }

    pub fn check_interrupts(&mut self) -> (bool, bool) {
        let res = (
            self.registers.interrupts.vblank,
            self.registers.interrupts.stat,
        );
        self.registers.interrupts.vblank = false;
        self.registers.interrupts.stat = false;
        res
    }

    pub fn draw(&mut self, cycles_passed: u64) {
        for _ in 0..cycles_passed / 4 {
            self.cycles += 4;
            if self.cycles > 70224 {
                self.cycles -= 70224;
                self.enable_display_events = true;
                self.display.draw(self.viewport);
                // self.display.draw(self.dump_tiles(0x8000));
            }
            self.step();
        }
    }

    fn step(&mut self) {
        let clocks = self.cycles % 456;
        let scanline = (self.cycles / 456) as u8;

        // Start of a line
        if scanline != self.registers.LY {
            self.block_stat_irqs = false;
            if scanline == 0 {
                self.registers.WC = 0;
            }
        }
        self.registers.LY = scanline;

        // Check for LY = LYC
        let coincidence = self.registers.LY == self.registers.LYC;
        if coincidence && clocks == 0 {
            self.req_stat_interrupt(6);
        }
        self.registers.STAT &= !(1 << 2);
        self.registers.STAT |= (coincidence as u8) << 2;

        // PPU Mode switching
        if self.registers.LY < 144 {
            match clocks {
                0 => self.set_mode(2), // OAM Search
                80 => {
                    self.set_mode(3); // Drawing
                    self.draw_line();
                }
                252 => self.set_mode(0), // H-blank
                _ => {}
            }
        }
        // V-blank
        else if self.registers.LY == 144 && clocks == 0 {
            self.registers.interrupts.vblank = true;
            self.set_mode(1);
        }
    }

    fn set_mode(&mut self, mode: u8) {
        self.registers.STAT &= !0b11;
        self.registers.STAT |= mode & 0b11;
        if mode < 3 {
            self.req_stat_interrupt(mode + 3);
        }
    }

    fn req_stat_interrupt(&mut self, bit: u8) {
        if !self.block_stat_irqs
            && (self.registers.STAT & (1 << bit)) != 0
            && (3..=6).contains(&bit)
        {
            self.block_stat_irqs = true;
            self.registers.interrupts.stat = true;
        }
    }

    fn draw_line(&mut self) {
        // LCD Enable
        if self.registers.LCDC & (1 << 7) != 0 {
            // bg/win enable
            if self.registers.LCDC & 1 != 0 {
                let bg_tilemap = match self.registers.LCDC & (1 << 3) != 0 {
                    true => 0x9C00,
                    false => 0x9800,
                };
                let bg_y = self.registers.SCY.wrapping_add(self.registers.LY);
                for i in 0u8..32 {
                    let bg_tile_num =
                        self.read_byte(bg_tilemap + 32 * ((bg_y / 8) as u16) + i as u16);
                    let bg_tile_row = self.decode_tile_row(bg_tile_num, bg_y % 8);
                    for j in 0u8..8 {
                        let bg_x = (8 * i + j).wrapping_sub(self.registers.SCX) as usize;
                        if bg_x < W_WIDTH {
                            self.viewport[self.registers.LY as usize][bg_x] =
                                self.decode_palette(bg_tile_row[j as usize]);
                        }
                    }
                }
            }
            if self.registers.LCDC & (1 << 5) != 0 && self.registers.LY >= self.registers.WY {
                let win_tilemap = match self.registers.LCDC & (1 << 6) != 0 {
                    true => 0x9C00,
                    false => 0x9800,
                };
                let mut window_visible = false;
                for i in 0u8..32 {
                    let win_tile_num = self
                        .read_byte(win_tilemap + 32 * ((self.registers.WC / 8) as u16) + i as u16);
                    let win_tile_row = self.decode_tile_row(win_tile_num, self.registers.WC % 8);
                    for j in 0..8 {
                        let win_x = 8 * i as usize + j + self.registers.WX as usize;
                        if win_x >= 7 && win_x < W_WIDTH {
                            window_visible = true;
                            self.viewport[self.registers.LY as usize][win_x - 7] =
                                self.decode_palette(win_tile_row[j]);
                        }
                    }
                }
                if window_visible {
                    self.registers.WC += 1
                }
            }
        }
        // TODO: Sprite rendering
    }

    fn dump_tiles(&self, base: u16) -> [[Color; 256]; 256] {
        let mut bg = [[Color::WHITE; 256]; 256];
        for i in 0..256 {
            let tile_addr = base + i * 16;
            let tile_y = i / 32;
            let tile_x = i % 32;
            for j in 0..8 {
                let hi = self.read_byte(tile_addr + 2 * j + 1);
                let lo = self.read_byte(tile_addr + 2 * j);
                for k in 0..8 {
                    bg[(8 * tile_y + j) as usize][(8 * tile_x + 7 - k) as usize] =
                        self.decode_palette((((hi >> k) & 1) << 1) | ((lo >> k) & 1));
                }
            }
        }
        bg
    }

    fn decode_tile_row(&self, tile_num: u8, row_num: u8) -> [u8; 8] {
        let mut row = [0; 8];
        let tile_row_offset = if self.registers.LCDC & (1 << 4) == 0 && tile_num <= 0x80 {
            0x9000
        } else {
            0x8000
        } + tile_num as u16 * 16
            + 2 * row_num as u16;
        let hi = self.read_byte(tile_row_offset + 1);
        let lo = self.read_byte(tile_row_offset);
        for i in 0..8 {
            row[7 - i] = (((hi >> i) & 1) << 1) | ((lo >> i) & 1);
        }
        row
    }

    fn decode_palette(&self, color: u8) -> Color {
        let color = (self.registers.BGP >> (2 * color)) & 0b11;
        match color {
            0 => Color::WHITE,
            1 => Color::RGB(0xaa, 0xaa, 0xaa),
            2 => Color::RGB(0x55, 0x55, 0x55),
            3 => Color::BLACK,
            _ => panic!("Incorrect palette color: {}", color),
        }
    }
}
