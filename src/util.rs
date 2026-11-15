//! Utilidades pequenas: leitura de arquivos com buffer reaproveitado e formatação PT-BR.
use std::fs::File;
use std::io::Read;

/// Lê o arquivo inteiro para `buf` (reaproveitando a memória). `false` se falhar.
pub fn read_into(path: &str, buf: &mut String) -> bool {
    buf.clear();
    match File::open(path) {
        Ok(mut f) => f.read_to_string(buf).is_ok(),
        Err(_) => false,
    }
}

pub fn read_int(path: &str, buf: &mut String) -> Option<i64> {
    if !read_into(path, buf) {
        return None;
    }
    buf.trim().parse().ok()
}

/// Número no formato brasileiro (vírgula decimal).
pub fn br(x: f64, nd: usize) -> String {
    format!("{:.*}", nd, x).replace('.', ",")
}

pub fn fmt_bytes(n: f64) -> String {
    const UNITS: [(&str, f64, usize); 3] = [
        ("GB", (1u64 << 30) as f64, 2),
        ("MB", (1u64 << 20) as f64, 1),
        ("KB", (1u64 << 10) as f64, 1),
    ];
    for (unit, div, nd) in UNITS {
        if n >= div {
            return format!("{} {}", br(n / div, nd), unit);
        }
    }
    format!("{} B", n as i64)
}

/// Arredondamento igual ao `round()` do Python (meio vai para o par).
pub fn pyround(x: f64) -> i64 {
    x.round_ties_even() as i64
}

pub fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// `str.center` do Python, cortado na largura.
pub fn center(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        return s.chars().take(w).collect();
    }
    let pad = w - n;
    let left = pad / 2 + (pad & w & 1);
    let mut out = String::with_capacity(w);
    out.extend(std::iter::repeat_n(' ', left));
    out.push_str(s);
    out.extend(std::iter::repeat_n(' ', pad - left));
    out
}

pub fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}
