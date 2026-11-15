//! Lista de processos: só /proc/PID/stat a cada atualização; uid e nome
//! completo ficam em cache por processo.
use crate::util::{read_into, round1};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub cpu: f64,
    pub rss: u64,
    pub memp: f64,
    pub nice: i32,
    pub threads: u32,
    pub uid: u32,
    /// 0 = pesado, 1 = ativo, 2 = segundo plano
    pub rank: u8,
}

struct Known {
    start: u64,
    ticks: u64,
    cpu: f64,
    uid: u32,
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

fn read_uid(pid: i32, path: &mut String, buf: &mut String) -> u32 {
    path.clear();
    let _ = write!(path, "/proc/{pid}/status");
    if read_into(path, buf) {
        for l in buf.lines() {
            if let Some(v) = l.strip_prefix("Uid:") {
                return v.split_ascii_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
            }
        }
    }
    0
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

    pub fn update(&mut self, total_ram: u64) {
        let now = Instant::now();
        let dt = self.last.map(|t| now.duration_since(t).as_secs_f64());
        // atualização logo depois da anterior: mantém o CPU% já calculado
        let quick = dt.is_some_and(|d| d < 0.5);
        if !quick {
            self.last = Some(now);
        }
        self.stamp += 1;
        let stamp = self.stamp;
        let refresh_uid = stamp.is_multiple_of(10);
        self.list.clear();
        let Ok(rd) = std::fs::read_dir("/proc") else { return };
        for e in rd.flatten() {
            let fname = e.file_name();
            let Some(pid) = fname.to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
            self.path.clear();
            let _ = write!(self.path, "/proc/{pid}/stat");
            if !read_into(&self.path, &mut self.buf) {
                continue;
            }
            let b = self.buf.as_str();
            let (Some(l), Some(r)) = (b.find('('), b.rfind(')')) else { continue };
            if r < l {
                continue;
            }
            let comm = &b[l + 1..r];
            let mut it = b[r + 1..].split_ascii_whitespace();
            let state = it.next().map(|s| s.as_bytes()[0]).unwrap_or(b'?');
            let ppid: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let utime: u64 = it.nth(9).and_then(|v| v.parse().ok()).unwrap_or(0);
            let stime: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let nice: i32 = it.nth(3).and_then(|v| v.parse().ok()).unwrap_or(0);
            let threads: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let start: u64 = it.nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
            let rss_pages: u64 = it.nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
            let ticks = utime + stime;

            let k = match self.known.get_mut(&pid) {
                Some(k) if k.start == start => {
                    if !quick {
                        k.cpu = match dt {
                            Some(d) if d > 0.0 => round1(ticks.saturating_sub(k.ticks) as f64 / (d * self.clk) * 100.0),
                            _ => 0.0,
                        };
                        k.ticks = ticks;
                    }
                    if refresh_uid {
                        k.uid = read_uid(pid, &mut self.path, &mut self.buf2);
                    }
                    k
                }
                _ => {
                    let uid = read_uid(pid, &mut self.path, &mut self.buf2);
                    self.known.insert(pid, Known { start, ticks, cpu: 0.0, uid, ext: None, stamp });
                    self.known.get_mut(&pid).unwrap()
                }
            };
            k.stamp = stamp;
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
            let rss = rss_pages * self.page;
            let memp = if total_ram > 0 { 100.0 * rss as f64 / total_ram as f64 } else { 0.0 };
            let rank = if cpu >= 50.0 || memp >= 10.0 {
                0
            } else if cpu >= 0.5 || state == b'R' {
                1
            } else {
                2
            };
            self.users.entry(uid).or_insert_with(|| user_name(uid));
            self.list.push(Proc { pid, ppid, name, cpu, rss, memp, nice, threads, uid, rank });
        }
        self.known.retain(|_, k| k.stamp == stamp);
        self.list.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then(b.cpu.total_cmp(&a.cpu))
                .then(b.memp.total_cmp(&a.memp))
                .then(a.pid.cmp(&b.pid))
        });
    }
}
