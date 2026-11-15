//! Todas as cores (copiadas do CSS da versão em Python).
use ratatui::style::Color;

pub const fn rgb(h: u32) -> Color {
    Color::Rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

pub const BG: Color = rgb(0x0a0f1a);
pub const BAR_BG: Color = rgb(0x0f1a2b);
pub const TAB_OFF: Color = rgb(0x8fb8d8);
pub const TAB_ON: Color = rgb(0xe8f6ff);
pub const ACCENT: Color = rgb(0x5ec8ff);
pub const HEAD_BG: Color = rgb(0x1b2a40);
pub const TITLE: Color = rgb(0xe8f6ff);
pub const SUB: Color = rgb(0x9fd0f5);
pub const SUB2: Color = rgb(0x6f93b3);
pub const DET: Color = rgb(0xc9dcec);

pub const METER_LIT: Color = rgb(0x2bff2b);
pub const METER_OFF: Color = rgb(0x0c3a12);
pub const METER_NAME: Color = rgb(0x7fa8c8);
pub const METER_BORDER: Color = rgb(0x2e4a66);
pub const RAM_BORDER: Color = rgb(0x3f6a4a);
pub const ZRAM_LIT: Color = rgb(0x5ec8ff);
pub const ZRAM_OFF: Color = rgb(0x0b2b3d);
pub const ZRAM_BORDER: Color = rgb(0x2e5a7a);
pub const BLACK: Color = rgb(0x000000);

pub const GPU_CARD_BG: Color = rgb(0x16141f);
pub const GPU_BORDER: Color = rgb(0x3d3452);
pub const GPU_TITLE: Color = rgb(0xe6d6ff);
pub const GPU_GRAPH_BG: Color = rgb(0x121019);
pub const VRAM_GRAPH_BG: Color = rgb(0x10141b);
pub const GPU_LBL: Color = rgb(0x9c94b0);
pub const GPU_BIG: Color = rgb(0xf3e9ff);
pub const GPU_VAL: Color = rgb(0xe8e0f5);
/// (preenchimento, topo, grade)
pub const PAL_MODERN: [Color; 3] = [rgb(0x5a3f78), rgb(0xd7a6f0), rgb(0x2a2338)];
pub const PAL_MODERN2: [Color; 3] = [rgb(0x1f4f6e), rgb(0x8fd3ff), rgb(0x1f2a38)];

pub const OV_BG: Color = rgb(0x0f1724);
pub const TABLE_BG: Color = rgb(0x0c121d);
pub const TABLE_ALT: Color = rgb(0x121b2a);
pub const CURSOR_BG: Color = rgb(0x24527a);
pub const TEXT: Color = rgb(0xe0e0e0);
pub const SOFT: Color = rgb(0xd8e6f2);
pub const OTHER_USER: Color = rgb(0x9aa8b6);
pub const WHITE: Color = rgb(0xffffff);
pub const ORANGE: Color = rgb(0xffb347);
pub const GREEN: Color = rgb(0x3ddc54);
pub const GRAY: Color = rgb(0x7d8794);
pub const RED: Color = rgb(0xff4a3d);

pub const DIALOG_BG: Color = rgb(0x16202f);
pub const BTN_ERR: Color = rgb(0xba3c5b);
pub const BTN_ERR_OFF: Color = rgb(0x4a2531);
pub const BTN_PRI: Color = rgb(0x0178d4);
pub const BTN_PRI_OFF: Color = rgb(0x1c3550);
pub const FOOT_KEY: Color = rgb(0xffa62b);
pub const INPUT_BG: Color = rgb(0x111a28);
pub const PLACEHOLDER: Color = rgb(0x6f8399);
