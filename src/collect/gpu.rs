//! GPU amdgpu: uso pelos contadores de tempo do DRM fdinfo (contagem exata,
//! não amostragem — gpu_busy_percent nessa placa pulsa e dá número aleatório).
use crate::util::{read_int, read_into};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

const RESCAN_TICKS: u32 = 5;

#[derive(Default, Clone)]
pub struct Sensors {
    pub temp: Option<i64>,
    pub sclk: Option<i64>,
    pub mclk: Option<i64>,
    pub fan: Option<i64>,
    pub power: Option<i64>,
    pub cap: Option<i64>,
    pub vram_used: Option<i64>,
    pub vram_total: Option<i64>,
}

pub struct Gpu {
    pub ok: bool,
    dev: String,
    hwmon: Option<String>,
    pdev: String,
    pub name: String,
    paths: Vec<String>,
    prev: Option<HashMap<u64, u64>>,
    spare: HashMap<u64, u64>,
    prev_t: Instant,
    tick: u32,
    buf: String,
    pub sens: Sensors,
}

fn sorted_dir(dir: &str, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix))
        .collect();
    v.sort();
    v
}

impl Gpu {
    pub fn new() -> Self {
        let mut g = Gpu {
            ok: false,
            dev: String::new(),
            hwmon: None,
            pdev: String::new(),
            name: "GPU AMD (amdgpu) não encontrada".into(),
            paths: Vec::new(),
            prev: None,
            spare: HashMap::new(),
            prev_t: Instant::now(),
            tick: 0,
            buf: String::new(),
            sens: Sensors::default(),
        };
        for card in sorted_dir("/sys/class/drm", "card") {
            if card.contains('-') || !card[4..].starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let dev = format!("/sys/class/drm/{card}/device");
            read_into(&format!("{dev}/uevent"), &mut g.buf);
            if !g.buf.lines().any(|l| l == "DRIVER=amdgpu") {
                continue;
            }
            g.pdev = std::fs::canonicalize(&dev)
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            g.hwmon = sorted_dir(&format!("{dev}/hwmon"), "hwmon").first().map(|h| format!("{dev}/hwmon/{h}"));
            g.dev = dev;
            g.ok = true;
            break;
        }
        if g.ok {
            g.name = g.lspci_name();
        }
        g
    }

    fn lspci_name(&self) -> String {
        let out = std::process::Command::new("lspci").args(["-mm", "-s", &self.pdev]).output();
        let Ok(out) = out else { return "GPU AMD".into() };
        let txt = String::from_utf8_lossy(&out.stdout);
        // campos entre aspas: classe, fabricante, dispositivo, ...
        let Some(dev) = txt.split('"').skip(1).step_by(2).nth(2) else { return "GPU AMD".into() };
        let code = dev.split('[').next().unwrap_or("").trim();
        let (Some(a), Some(b)) = (dev.find('['), dev.rfind(']')) else { return format!("AMD {dev}") };
        if b < a {
            return format!("AMD {dev}");
        }
        let inner = &dev[a + 1..b];
        let name = inner.rsplit(" / ").next().unwrap_or(inner);
        let name = if name.starts_with("Radeon") { name.to_string() } else { format!("Radeon {name}") };
        if code.is_empty() { format!("AMD {name}") } else { format!("AMD {name}  ({code})") }
    }

    /// Uso da GPU (0–100) desde a última chamada. `None` na primeira vez.
    pub fn sample(&mut self) -> Option<f64> {
        if !self.ok {
            return None;
        }
        if self.tick.is_multiple_of(RESCAN_TICKS) {
            self.rescan();
        }
        self.tick = self.tick.wrapping_add(1);

        let mut cur = std::mem::take(&mut self.spare);
        cur.clear();
        self.read_clients(&mut cur);
        let now = Instant::now();
        let dt = now.duration_since(self.prev_t).as_nanos() as f64;
        self.prev_t = now;
        let prev = self.prev.replace(cur);
        let result = match (&prev, &self.prev) {
            (Some(prev), Some(cur)) if dt > 0.0 => {
                let mut busy = 0u64;
                for (cid, ns) in cur {
                    if let Some(p) = prev.get(cid)
                        && ns >= p {
                            busy += ns - p; // cliente novo só vira base
                        }
                }
                Some((100.0 * busy as f64 / dt).min(100.0))
            }
            _ => None,
        };
        self.spare = prev.unwrap_or_default();
        result
    }

    fn read_clients(&mut self, out: &mut HashMap<u64, u64>) {
        let buf = &mut self.buf;
        let pdev = &self.pdev;
        self.paths.retain(|path| {
            if !read_into(path, buf) {
                return false; // fd fechado / processo saiu
            }
            let (mut drv, mut pd_ok, mut cid, mut ns) = (false, true, None, 0u64);
            for line in buf.lines() {
                let Some(rest) = line.strip_prefix("drm-") else { continue };
                if let Some(v) = rest.strip_prefix("driver:") {
                    drv = v.trim() == "amdgpu";
                } else if let Some(v) = rest.strip_prefix("pdev:") {
                    if let Some(p) = v.split_ascii_whitespace().next() {
                        pd_ok = pdev.is_empty() || p == pdev;
                    }
                } else if let Some(v) = rest.strip_prefix("client-id:") {
                    cid = v.trim().parse::<u64>().ok();
                } else if let Some(v) = rest.strip_prefix("engine-gfx:").or_else(|| rest.strip_prefix("engine-compute:")) {
                    ns += v.split_ascii_whitespace().next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
                }
            }
            if drv && pd_ok
                && let Some(cid) = cid {
                    let e = out.entry(cid).or_insert(0);
                    *e = (*e).max(ns); // fds duplicados do mesmo cliente
                }
            true
        });
    }

    fn rescan(&mut self) {
        self.paths.clear();
        let Ok(rd) = std::fs::read_dir("/proc") else { return };
        let mut p = String::new();
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(pid) = name.to_str().filter(|s| s.as_bytes()[0].is_ascii_digit()) else { continue };
            p.clear();
            let _ = write!(p, "/proc/{pid}/fd");
            let Ok(fds) = std::fs::read_dir(&p) else { continue };
            for fd in fds.flatten() {
                if let Ok(target) = std::fs::read_link(fd.path())
                    && target.as_os_str().as_encoded_bytes().starts_with(b"/dev/dri/")
                        && let Some(fdn) = fd.file_name().to_str() {
                            self.paths.push(format!("/proc/{pid}/fdinfo/{fdn}"));
                        }
            }
        }
    }

    /// Lê os sensores. `all` = todos; senão só VRAM (para o histórico) e,
    /// se `temp`, a temperatura.
    pub fn read_sensors(&mut self, all: bool, temp: bool) {
        if !self.ok {
            return;
        }
        let b = &mut self.buf;
        let d = &self.dev;
        self.sens.vram_used = read_int(&format!("{d}/mem_info_vram_used"), b);
        self.sens.vram_total = read_int(&format!("{d}/mem_info_vram_total"), b);
        let Some(h) = &self.hwmon else { return };
        if all || temp {
            self.sens.temp = read_int(&format!("{h}/temp1_input"), b);
        }
        if all {
            self.sens.sclk = read_int(&format!("{h}/freq1_input"), b);
            self.sens.mclk = read_int(&format!("{h}/freq2_input"), b);
            self.sens.fan = read_int(&format!("{h}/fan1_input"), b);
            self.sens.power = read_int(&format!("{h}/power1_input"), b)
                .filter(|v| *v != 0)
                .or_else(|| read_int(&format!("{h}/power1_average"), b));
            self.sens.cap = read_int(&format!("{h}/power1_cap"), b);
        }
    }
}
