//! Fontes grandes de 3 e 2 linhas, copiadas caractere por caractere.

fn glyph3(c: char) -> [&'static str; 3] {
    match c {
        'C' => ["█▀▀▀", "█   ", "▀▀▀▀"],
        'P' => ["█▀▀█", "█▀▀▀", "▀   "],
        'U' => ["█  █", "█  █", "▀▀▀▀"],
        'R' => ["█▀▀█", "█▀▀▄", "▀  ▀"],
        'A' => ["█▀▀█", "█▀▀█", "▀  ▀"],
        'M' => ["█▄ ▄█", "█ ▀ █", "▀   ▀"],
        'G' => ["█▀▀▀", "█ ▀█", "▀▀▀▀"],
        'O' => ["█▀▀█", "█  █", "▀▀▀▀"],
        'S' => ["█▀▀▀", "▀▀▀█", "▀▀▀▀"],
        'E' => ["█▀▀▀", "█▀▀ ", "▀▀▀▀"],
        '0' => ["█▀█", "█ █", "▀▀▀"],
        '1' => ["▀█ ", " █ ", "▀▀▀"],
        '2' => ["▀▀█", "█▀▀", "▀▀▀"],
        '3' => ["▀▀█", " ▀█", "▀▀▀"],
        '4' => ["█ █", "▀▀█", "  ▀"],
        '5' => ["█▀▀", "▀▀█", "▀▀▀"],
        '6' => ["█▀▀", "█▀█", "▀▀▀"],
        '7' => ["▀▀█", "  █", "  ▀"],
        '8' => ["█▀█", "█▀█", "▀▀▀"],
        '9' => ["█▀█", "▀▀█", "▀▀▀"],
        '%' => ["▀ ▄▀", " ▄▀ ", "▀  ▀"],
        '°' => ["█▀█", "▀▀▀", "   "],
        '-' => ["   ", "▀▀▀", "   "],
        ',' => [" ", " ", "▄"],
        _ => ["  ", "  ", "  "],
    }
}

fn glyph2(c: char) -> [&'static str; 2] {
    match c {
        'C' => ["█▀▀", "█▄▄"],
        'P' => ["█▀█", "█▀▀"],
        'U' => ["█ █", "█▄█"],
        'M' => ["█▄ ▄█", "█ ▀ █"],
        'E' => ["█▀▀", "██▄"],
        'O' => ["█▀█", "█▄█"],
        'R' => ["█▀█", "█▀▄"],
        'I' => ["█", "█"],
        'A' => ["█▀█", "█▀█"],
        'G' => ["█▀▀", "█▄█"],
        _ => [" ", " "],
    }
}

/// Texto grande de 3 linhas (números da aba GPU).
pub fn big(s: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for ch in s.to_uppercase().chars() {
        let g = glyph3(ch);
        for i in 0..3 {
            rows[i].push_str(g[i]);
            rows[i].push(' ');
        }
    }
    rows.map(|r| r.trim_end().to_string())
}

/// Título de 2 linhas ("CPU", "MEMORIA").
pub fn title(s: &str) -> [String; 2] {
    let mut rows = [String::new(), String::new()];
    for ch in s.to_uppercase().chars() {
        let g = glyph2(ch);
        for i in 0..2 {
            rows[i].push_str(g[i]);
            rows[i].push(' ');
        }
    }
    rows.map(|r| r.trim_end().to_string())
}
