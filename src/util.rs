//! Utilidades pequenas: leitura de arquivos com buffer reaproveitado e formatação PT-BR.
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

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
    const UNITS: [(&str, f64, usize); 3] = [("GB", (1u64 << 30) as f64, 2), ("MB", (1u64 << 20) as f64, 1), ("KB", (1u64 << 10) as f64, 1)];
    for (unit, div, nd) in UNITS {
        if n >= div {
            return format!("{} {}", br(n / div, nd), unit);
        }
    }
    format!("{} B", n as i64)
}

/// Tempo ligado curto: "3 d 4 h", "4 h 12 min", "12 min".
pub fn fmt_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
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

/// Abre um pidfd: uma referência ao processo que não passa a apontar para
/// outro se o PID for reaproveitado (Linux 5.3+). `Err` traz o errno.
pub fn pidfd_open(pid: i32) -> Result<OwnedFd, i32> {
    // SAFETY: a syscall só recebe inteiros.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(errno());
    }
    // SAFETY: a syscall acabou de criar esse fd e ninguém mais o tem.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// Manda o sinal `sig` pelo pidfd. `Err` traz o errno.
pub fn pidfd_kill(fd: &OwnedFd, sig: i32) -> Result<(), i32> {
    // SAFETY: fd é um pidfd válido enquanto `fd` existir; siginfo nulo = igual ao kill().
    let r = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd.as_raw_fd(), sig, std::ptr::null::<libc::siginfo_t>(), 0) };
    if r == 0 { Ok(()) } else { Err(errno()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn br_usa_virgula() {
        assert_eq!(br(2.34, 1), "2,3");
        assert_eq!(br(2.0, 2), "2,00");
    }

    #[test]
    fn fmt_bytes_escolhe_a_unidade() {
        assert_eq!(fmt_bytes(512.0), "512 B");
        assert_eq!(fmt_bytes(1536.0), "1,5 KB");
        assert_eq!(fmt_bytes(5.0 * 1048576.0), "5,0 MB");
        assert_eq!(fmt_bytes(3.0 * 1073741824.0), "3,00 GB");
    }

    #[test]
    fn fmt_uptime_mostra_as_duas_maiores_unidades() {
        assert_eq!(fmt_uptime(59), "0 min");
        assert_eq!(fmt_uptime(4 * 3600 + 12 * 60 + 30), "4 h 12 min");
        assert_eq!(fmt_uptime(3 * 86_400 + 4 * 3600 + 59 * 60), "3 d 4 h");
    }

    #[test]
    fn pyround_arredonda_meio_para_o_par() {
        assert_eq!([0.5, 1.5, 2.5, 2.6, -0.5].map(pyround), [0, 2, 2, 3, 0]);
    }

    #[test]
    fn center_igual_ao_python() {
        // mesmos resultados de "ab".center(5), "abc".center(6), etc.
        assert_eq!(center("ab", 5), "  ab ");
        assert_eq!(center("abc", 6), " abc  ");
        assert_eq!(center("abc", 5), " abc ");
        assert_eq!(center("abcdef", 3), "abc");
        assert_eq!(center("ação", 6), " ação ");
    }

    #[test]
    fn pidfd_encerra_o_processo_certo() {
        let mut filho = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let fd = pidfd_open(filho.id() as i32).unwrap();
        pidfd_kill(&fd, libc::SIGTERM).unwrap();
        let status = filho.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGTERM));
        // depois de recolhido, o pidfd não encontra mais o processo
        assert_eq!(pidfd_kill(&fd, libc::SIGTERM), Err(libc::ESRCH));
    }
}
