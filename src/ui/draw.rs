//! Ferramentas de desenho direto no Buffer do ratatui, sempre com recorte
//! (nada aqui pode dar panic com terminal pequeno), mais os dois widgets
//! próprios: o medidor estilo Windows 7 e o gráfico de histórico.
use crate::ui::theme::*;
use crate::util::{center, pyround};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use std::collections::VecDeque;

#[derive(Clone, Copy, Default)]
pub struct R {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl R {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> R {
        R { x, y, w: w.max(0), h: h.max(0) }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inter(&self, o: R) -> R {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        R::new(x, y, self.right().min(o.right()) - x, self.bottom().min(o.bottom()) - y)
    }
    pub fn shrink(&self, dx: i32, dy: i32) -> R {
        R::new(self.x + dx, self.y + dy, self.w - 2 * dx, self.h - 2 * dy)
    }
}

pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}
pub fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

pub fn width(s: &str) -> i32 {
    s.chars().count() as i32
}

pub struct Cv<'a> {
    pub buf: &'a mut Buffer,
    pub clip: R,
}

impl Cv<'_> {
    pub fn clipped(&mut self, r: R) -> Cv<'_> {
        Cv { clip: self.clip.inter(r), buf: &mut *self.buf }
    }

    pub fn ch(&mut self, x: i32, y: i32, c: char, st: Style) {
        if self.clip.contains(x, y)
            && let Some(cell) = self.buf.cell_mut((x as u16, y as u16)) {
                cell.set_char(c).set_style(st);
            }
    }

    /// Escreve e devolve a coluna seguinte.
    pub fn put(&mut self, x: i32, y: i32, s: &str, st: Style) -> i32 {
        let mut cx = x;
        for c in s.chars() {
            self.ch(cx, y, c, st);
            cx += 1;
        }
        cx
    }

    /// Escreve dentro de uma largura fixa (corta o excesso; `right` alinha à direita).
    pub fn put_w(&mut self, x: i32, y: i32, w: i32, s: &str, st: Style, right: bool) {
        let n = width(s);
        let off = if right && n < w { w - n } else { 0 };
        let mut sub = self.clipped(R::new(x, y, w, 1));
        sub.put(x + off, y, s, st);
    }

    pub fn fill(&mut self, r: R, bg: Color) {
        let r = self.clip.inter(r);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                if let Some(cell) = self.buf.cell_mut((x as u16, y as u16)) {
                    cell.set_char(' ').set_bg(bg);
                }
            }
        }
    }

    /// Borda arredondada.
    pub fn frame(&mut self, r: R, color: Color) {
        if r.w < 2 || r.h < 2 {
            return;
        }
        let st = fg(color);
        let (x1, y1) = (r.right() - 1, r.bottom() - 1);
        for x in r.x + 1..x1 {
            self.ch(x, r.y, '─', st);
            self.ch(x, y1, '─', st);
        }
        for y in r.y + 1..y1 {
            self.ch(r.x, y, '│', st);
            self.ch(x1, y, '│', st);
        }
        self.ch(r.x, r.y, '╭', st);
        self.ch(x1, r.y, '╮', st);
        self.ch(r.x, y1, '╰', st);
        self.ch(x1, y1, '╯', st);
    }
}

/// Medidor vertical de barrinhas (Gerenciador de Tarefas do Windows 7).
#[allow(clippy::too_many_arguments)]
pub fn meter(cv: &mut Cv, r: R, value: f64, caption: &str, name: &str, lit: Color, off: Color, border: Color) {
    cv.fill(r, BLACK);
    cv.frame(r, border);
    let inner = r.shrink(1, 1);
    let mut cv = cv.clipped(inner);
    let (w, h) = (inner.w, inner.h);
    if w <= 0 || h <= 0 {
        return;
    }
    let bars = (h - 3).max(1);
    let mut n_lit = pyround(value.clamp(0.0, 100.0) / 100.0 * bars as f64) as i32;
    if value > 0.0 && n_lit == 0 {
        n_lit = 1;
    }
    let seg = ((w - 3) / 2).max(1);
    let left = (w - (seg * 2 + 1)) / 2;
    for row in 0..bars {
        let color = if bars - row <= n_lit { lit } else { off };
        let y = inner.y + row;
        for k in 0..seg {
            cv.ch(inner.x + left + k, y, '▄', fg(color));
            cv.ch(inner.x + left + seg + 1 + k, y, '▄', fg(color));
        }
    }
    cv.put(inner.x, inner.y + bars + 1, &center(caption, w as usize), bold(lit));
    cv.put(inner.x, inner.y + bars + 2, &center(name, w as usize), fg(METER_NAME));
}

/// Gráfico de histórico em área (estilo Windows 11), com resolução de 1/8 de linha.
pub fn graph(cv: &mut Cv, r: R, data: &VecDeque<f64>, pal: [Color; 3], bg: Color) {
    const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    cv.fill(r, bg);
    let (w, h) = (r.w, r.h);
    if w <= 0 || h <= 0 {
        return;
    }
    let [fill, top, grid] = pal;
    let n = data.len() as i32;
    for x in 0..w {
        // mostra só as últimas `w` amostras, encostadas à direita
        let di = n - w + x;
        let v = if di >= 0 { data[di as usize] } else { 0.0 };
        let eighths = pyround(v / 100.0 * h as f64 * 8.0) as i32;
        let (full, part) = (eighths / 8, eighths % 8);
        for row in 0..h {
            let idx = h - 1 - row; // 0 = linha de baixo
            let (c, color) = if idx < full {
                ('█', if idx == full - 1 && part == 0 { top } else { fill })
            } else if idx == full && part > 0 {
                (EIGHTHS[part as usize], top)
            } else {
                (if idx == 0 { '▁' } else { ' ' }, grid)
            };
            cv.ch(r.x + x, r.y + row, c, fg(color));
        }
    }
}

/// Quebra de linha por palavras, mantendo os espaços duplos do texto.
pub fn wrap(s: &str, w: i32) -> Vec<String> {
    if width(s) <= w || w <= 0 {
        return vec![s.to_string()];
    }
    let mut lines = vec![String::new()];
    let mut rest = s;
    while !rest.is_empty() {
        // próximo pedaço = espaços + palavra
        let sp = rest.len() - rest.trim_start_matches(' ').len();
        let end = rest[sp..].find(' ').map(|i| sp + i).unwrap_or(rest.len());
        let (gap, word) = (&rest[..sp], &rest[sp..end]);
        rest = &rest[end..];
        let cur = lines.last_mut().unwrap();
        if cur.is_empty() {
            cur.push_str(word);
        } else if width(cur) + width(gap) + width(word) <= w {
            cur.push_str(gap);
            cur.push_str(word);
        } else if !word.is_empty() {
            lines.push(word.to_string());
        }
    }
    lines
}
