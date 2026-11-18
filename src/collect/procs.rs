//! Lista de processos: só /proc/PID/stat a cada atualização; uid e nome
//! completo ficam em cache por processo.
use crate::util::{read_into, round1};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write as _;
use std::str::{FromStr, SplitAsciiWhitespace};
use std::time::Instant;

/// Grupo do processo na lista (a ordem das variantes é a ordem na tela).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// CPU ≥ 50% de um núcleo ou RAM ≥ 10%
    Pesado,
    /// usando CPU agora ou na fila para rodar
    Ativo,
    SegundoPlano,
}

impl Rank {
    pub fn of(cpu: f64, memp: f64, state: u8) -> Rank {
        if cpu >= 50.0 || memp >= 10.0 {
            Rank::Pesado
        } else if cpu >= 0.5 || state == b'R' {
            Rank::Ativo
        } else {
            Rank::SegundoPlano
        }
    }
}

pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    /// momento em que o processo nasceu (em ticks desde o boot): junto com o
    /// PID, identifica o processo mesmo que o número seja reaproveitado
    pub start: u64,
    pub name: String,
    pub cpu: f64,
    pub rss: u64,
    pub memp: f64,
    pub nice: i32,
    pub threads: u32,
    /// `None` quando não deu para ler o dono (o processo acabou de sair, etc.)
    pub uid: Option<u32>,
    pub rank: Rank,
}

/// Campos de /proc/PID/stat que o AeroTop usa.
#[derive(Debug, PartialEq, Eq)]
pub struct Stat<'a> {
    pub comm: &'a str,
    pub state: u8,
    pub ppid: i32,
    /// utime + stime
    pub ticks: u64,
    pub nice: i32,
    pub threads: u32,
    pub start: u64,
    pub rss_pages: u64,
}

fn field<T: FromStr + Default>(it: &mut SplitAsciiWhitespace<'_>, skip: usize) -> T {
    it.nth(skip).and_then(|v| v.parse().ok()).unwrap_or_default()
}

/// Lê uma linha de /proc/PID/stat. O nome (campo 2) fica entre parênteses e
/// pode ter espaços e parênteses, por isso o fim dele é o último `)`.
pub fn parse_stat(b: &str) -> Option<Stat<'_>> {
    let (l, r) = (b.find('(')?, b.rfind(')')?);
    if r < l {
        return None;
    }
    let mut it = b[r + 1..].split_ascii_whitespace();
    // campos contados a partir do 3 (state); ver `man 5 proc_pid_stat`
    let state = it.next().map_or(b'?', |s| s.as_bytes()[0]);
    let ppid = field(&mut it, 0); // 4
    let utime: u64 = field(&mut it, 9); // 14
    let stime: u64 = field(&mut it, 0); // 15
    let nice = field(&mut it, 3); // 19
    let threads = field(&mut it, 0); // 20
    let start = field(&mut it, 1); // 22
    let rss_pages = field(&mut it, 1); // 24
    Some(Stat { comm: &b[l + 1..r], state, ppid, ticks: utime + stime, nice, threads, start, rss_pages })
}

/// Momento de nascimento do processo `pid` agora (ver [`Proc::start`]).
pub fn start_time(pid: i32) -> Option<u64> {
    let mut buf = String::new();
    read_into(&format!("/proc/{pid}/stat"), &mut buf).then_some(())?;
    parse_stat(&buf).map(|s| s.start)
}

/// Uso de CPU em % de um núcleo: `ticks` de CPU gastos em `dt` segundos.
fn cpu_pct(ticks: u64, dt: f64, clk: f64) -> f64 {
    if dt > 0.0 { round1(ticks as f64 / (dt * clk) * 100.0) } else { 0.0 }
}

struct Known {
    start: u64,
    ticks: u64,
    cpu: f64,
    uid: Option<u32>,
    ext: Option<(String, String)>, // (comm cortado, nome completo)
    stamp: u64,
}

pub struct Procs {
    known: HashMap<i32, Known>,
    pub list: Vec<Proc>,
    pub users: HashMap<u32, String>,
    last: Option<Instant>,
    stamp: u64,
    clk: f64,
    page: u64,
    buf: String,
    buf2: String,
    path: String,
}

/// uid real, da linha `Uid:` de um /proc/PID/status
fn parse_uid(status: &str) -> Option<u32> {
    let line = status.lines().find_map(|l| l.strip_prefix("Uid:"))?;
    line.split_ascii_whitespace().next()?.parse().ok()
}

fn read_uid(pid: i32, path: &mut String, buf: &mut String) -> Option<u32> {
    path.clear();
    let _ = write!(path, "/proc/{pid}/status");
    if read_into(path, buf) { parse_uid(buf) } else { None }
}

/// Como o psutil: se o nome veio cortado em 15 letras, tenta completar pelo cmdline.
fn full_name(pid: i32, comm: &str, path: &mut String) -> String {
    path.clear();
    let _ = write!(path, "/proc/{pid}/cmdline");
    if let Ok(bytes) = std::fs::read(&*path) {
        let first = bytes.split(|b| *b == 0).next().unwrap_or(&[]);
        let first = String::from_utf8_lossy(first);
        let base = first.rsplit('/').next().unwrap_or("");
        if base.starts_with(comm) {
            return base.to_string();
        }
    }
    comm.to_string()
}

fn user_name(uid: u32) -> String {
    // SAFETY: getpwuid devolve ponteiro para área estática; só é chamado na thread principal.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() || (*pw).pw_name.is_null() {
            return uid.to_string();
        }
        std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
    }
}

impl Procs {
    pub fn new() -> Self {
        // SAFETY: sysconf não tem pré-condições.
        let (clk, page) = unsafe { (libc::sysconf(libc::_SC_CLK_TCK), libc::sysconf(libc::_SC_PAGESIZE)) };
        Procs {
            known: HashMap::new(),
            list: Vec::new(),
            users: HashMap::new(),
            last: None,
            stamp: 0,
            clk: if clk > 0 { clk as f64 } else { 100.0 },
            page: if page > 0 { page as u64 } else { 4096 },
            buf: String::new(),
            buf2: String::new(),
            path: String::new(),
        }
    }

    /// Relê a lista. Devolve `false` quando o CPU% não foi recalculado (leitura
    /// colada na anterior, ou a primeira de todas, que só serve de base).
    pub fn update(&mut self, total_ram: u64) -> bool {
        let now = Instant::now();
        let dt = self.last.map(|t| now.duration_since(t).as_secs_f64());
        // atualização logo depois da anterior: mantém o CPU% já calculado
        let quick = dt.is_some_and(|d| d < 0.5);
        if !quick {
            self.last = Some(now);
        }
        // muito tempo desde a anterior (a aba estava fechada): a média seria
        // velha, então essa leitura só vira base
        let dt = dt.filter(|d| *d <= 5.0);
        self.stamp += 1;
        let stamp = self.stamp;
        let refresh_uid = stamp.is_multiple_of(10);
        self.list.clear();
        let Ok(rd) = std::fs::read_dir("/proc") else { return false };
        for e in rd.flatten() {
            let fname = e.file_name();
            let Some(pid) = fname.to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
            self.path.clear();
            let _ = write!(self.path, "/proc/{pid}/stat");
            if !read_into(&self.path, &mut self.buf) {
                continue;
            }
            let Some(st) = parse_stat(&self.buf) else { continue };

            let k = match self.known.entry(pid) {
                Entry::Occupied(o) if o.get().start == st.start => {
                    let k = o.into_mut();
                    if !quick {
                        k.cpu = cpu_pct(st.ticks.saturating_sub(k.ticks), dt.unwrap_or(0.0), self.clk);
                        k.ticks = st.ticks;
                    }
                    if refresh_uid {
                        k.uid = read_uid(pid, &mut self.path, &mut self.buf2).or(k.uid);
                    }
                    k
                }
                // processo novo (ou PID reaproveitado por outro processo)
                e => {
                    let uid = read_uid(pid, &mut self.path, &mut self.buf2);
                    let k = Known { start: st.start, ticks: st.ticks, cpu: 0.0, uid, ext: None, stamp };
                    match e {
                        Entry::Occupied(mut o) => {
                            o.insert(k);
                            o.into_mut()
                        }
                        Entry::Vacant(v) => v.insert(k),
                    }
                }
            };
            k.stamp = stamp;
            let comm = st.comm;
            let name = if comm.len() >= 15 {
                match &k.ext {
                    Some((c, full)) if c == comm => full.clone(),
                    _ => {
                        let full = full_name(pid, comm, &mut self.path);
                        k.ext = Some((comm.to_string(), full.clone()));
                        full
                    }
                }
            } else {
                comm.to_string()
            };
            let (cpu, uid) = (k.cpu, k.uid);
            let rss = st.rss_pages * self.page;
            let memp = if total_ram > 0 { 100.0 * rss as f64 / total_ram as f64 } else { 0.0 };
            if let Some(uid) = uid {
                self.users.entry(uid).or_insert_with(|| user_name(uid));
            }
            self.list.push(Proc {
                pid,
                ppid: st.ppid,
                start: st.start,
                name,
                cpu,
                rss,
                memp,
                nice: st.nice,
                threads: st.threads,
                uid,
                rank: Rank::of(cpu, memp, st.state),
            });
        }
        self.known.retain(|_, k| k.stamp == stamp);
        self.list.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then(b.cpu.total_cmp(&a.cpu))
                .then(b.memp.total_cmp(&a.memp))
                .then(a.pid.cmp(&b.pid))
        });
        dt.is_some() && !quick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // linha no formato do /proc/PID/stat (52 campos)
    const KITTY: &str = "4242 (kitty) S 1 4242 4242 0 -1 4194560 81234 0 12 0 1500 320 0 0 20 0 27 0 \
                         98765 1234567890 45678 18446744073709551615 1 1 0 0 0 0 0 4096 17663 0 0 0 17 3 0 0 0 0 0 \
                         0 0 0 0 0 0 0 0";

    #[test]
    fn parse_stat_le_os_campos_usados() {
        let st = parse_stat(KITTY).unwrap();
        assert_eq!(
            st,
            Stat { comm: "kitty", state: b'S', ppid: 1, ticks: 1820, nice: 0, threads: 27, start: 98765, rss_pages: 45678 }
        );
    }

    #[test]
    fn parse_stat_aceita_nome_com_espaco_e_parenteses() {
        let linha = KITTY.replacen("(kitty)", "(Web Content (x))", 1);
        let st = parse_stat(&linha).unwrap();
        assert_eq!(st.comm, "Web Content (x)");
        assert_eq!(st.ticks, 1820);
        assert_eq!(st.start, 98765);
    }

    #[test]
    fn parse_stat_recusa_linha_sem_nome() {
        assert_eq!(parse_stat("4242 kitty S 1"), None);
        assert_eq!(parse_stat("4242 )kitty( S 1"), None);
    }

    #[test]
    fn parse_uid_pega_o_uid_real() {
        let status = "Name:\tkitty\nState:\tS (sleeping)\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\n";
        assert_eq!(parse_uid(status), Some(1000));
        assert_eq!(parse_uid("Name:\tx\n"), None);
    }

    #[test]
    fn cpu_pct_em_porcento_de_um_nucleo() {
        assert_eq!(cpu_pct(100, 1.0, 100.0), 100.0);
        assert_eq!(cpu_pct(25, 2.0, 100.0), 12.5);
        assert_eq!(cpu_pct(25, 0.0, 100.0), 0.0);
    }

    #[test]
    fn rank_segue_os_limites_da_legenda() {
        assert_eq!(Rank::of(50.0, 0.0, b'S'), Rank::Pesado);
        assert_eq!(Rank::of(0.0, 10.0, b'S'), Rank::Pesado);
        assert_eq!(Rank::of(0.5, 0.0, b'S'), Rank::Ativo);
        assert_eq!(Rank::of(0.0, 0.0, b'R'), Rank::Ativo);
        assert_eq!(Rank::of(0.4, 9.9, b'S'), Rank::SegundoPlano);
        assert!(Rank::Pesado < Rank::Ativo && Rank::Ativo < Rank::SegundoPlano);
    }
}
