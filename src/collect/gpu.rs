//! GPU amdgpu: uso pelos contadores de tempo do DRM fdinfo (contagem exata,
//! não amostragem — gpu_busy_percent nessa placa pulsa e dá número aleatório).
use crate::util::{read_int, read_into};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Instant;

const RESCAN_TICKS: u32 = 5;
const SEM_GPU: &str = "GPU AMD (amdgpu) não encontrada";

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
    /// `None` quando não há placa com o driver amdgpu
    dev: Option<Dev>,
    pub name: String,
    pub sens: Sensors,
}

/// A placa encontrada e o estado da medição de uso.
struct Dev {
    /// /sys/class/drm/cardN/device
    dir: String,
    hwmon: Option<String>,
    /// endereço PCI (0000:01:00.0), para filtrar o fdinfo
    pdev: String,
    paths: Vec<String>,
    prev: Option<HashMap<u64, u64>>,
    spare: HashMap<u64, u64>,
    prev_t: Instant,
    tick: u32,
    buf: String,
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

/// Nome bonito a partir da saída de `lspci -mm -s <pdev>`.
fn lspci_name(txt: &str) -> Option<String> {
    // campos entre aspas: classe, fabricante, dispositivo, ...
    let dev = txt.split('"').skip(1).step_by(2).nth(2)?;
    let code = dev.split('[').next().unwrap_or("").trim();
    let (Some(a), Some(b)) = (dev.find('['), dev.rfind(']')) else { return Some(format!("AMD {dev}")) };
    if b < a {
        return Some(format!("AMD {dev}"));
    }
    let inner = &dev[a + 1..b];
    let name = inner.rsplit(" / ").next().unwrap_or(inner);
    let name = if name.starts_with("Radeon") { name.to_string() } else { format!("Radeon {name}") };
    Some(if code.is_empty() { format!("AMD {name}") } else { format!("AMD {name}  ({code})") })
}

/// Um arquivo fdinfo de DRM: `Some((client-id, ns de gfx+compute))` se for um
/// cliente amdgpu da placa `pdev` (vazio = qualquer placa).
fn parse_fdinfo(txt: &str, pdev: &str) -> Option<(u64, u64)> {
    let (mut drv, mut pd_ok, mut cid, mut ns) = (false, true, None, 0u64);
    for line in txt.lines() {
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
    if drv && pd_ok { cid.map(|c| (c, ns)) } else { None }
}

/// Tempo de GPU gasto entre duas leituras, só de clientes que já existiam
/// (cliente novo só vira base para a próxima).
fn busy_ns(prev: &HashMap<u64, u64>, cur: &HashMap<u64, u64>) -> u64 {
    cur.iter().filter_map(|(cid, ns)| ns.checked_sub(*prev.get(cid)?)).sum()
}

impl Gpu {
    pub fn new() -> Self {
        let dev = Dev::find();
        let name = dev
            .as_ref()
            .map(|d| {
                let out = std::process::Command::new("lspci").args(["-mm", "-s", &d.pdev]).output();
                out.ok().and_then(|o| lspci_name(&String::from_utf8_lossy(&o.stdout))).unwrap_or_else(|| "GPU AMD".into())
            })
            .unwrap_or_else(|| SEM_GPU.into());
        Gpu { dev, name, sens: Sensors::default() }
    }

    pub fn ok(&self) -> bool {
        self.dev.is_some()
    }

    /// Uso da GPU (0–100) desde a última chamada. `None` na primeira vez.
    pub fn sample(&mut self) -> Option<f64> {
        let d = self.dev.as_mut()?;
        if d.tick.is_multiple_of(RESCAN_TICKS) {
            d.rescan();
        }
        d.tick = d.tick.wrapping_add(1);

        let mut cur = std::mem::take(&mut d.spare);
        cur.clear();
        d.read_clients(&mut cur);
        let now = Instant::now();
        let dt = now.duration_since(d.prev_t).as_nanos() as f64;
        d.prev_t = now;
        let result = match &d.prev {
            Some(prev) if dt > 0.0 => Some((100.0 * busy_ns(prev, &cur) as f64 / dt).min(100.0)),
            _ => None,
        };
        // o mapa antigo vira o "reserva" da próxima leitura (sem alocar de novo)
        d.spare = d.prev.replace(cur).unwrap_or_default();
        result
    }

    /// Lê os sensores. `all` = todos; senão só VRAM (para o histórico) e,
    /// se `temp`, a temperatura.
    pub fn read_sensors(&mut self, all: bool, temp: bool) {
        let Some(d) = &mut self.dev else { return };
        let s = &mut self.sens;
        let b = &mut d.buf;
        let dir = &d.dir;
        s.vram_used = read_int(&format!("{dir}/mem_info_vram_used"), b);
        s.vram_total = read_int(&format!("{dir}/mem_info_vram_total"), b);
        let Some(h) = &d.hwmon else { return };
        if all || temp {
            s.temp = read_int(&format!("{h}/temp1_input"), b);
        }
        if all {
            s.sclk = read_int(&format!("{h}/freq1_input"), b);
            s.mclk = read_int(&format!("{h}/freq2_input"), b);
            s.fan = read_int(&format!("{h}/fan1_input"), b);
            s.power = read_int(&format!("{h}/power1_input"), b)
                .filter(|v| *v != 0)
                .or_else(|| read_int(&format!("{h}/power1_average"), b));
            s.cap = read_int(&format!("{h}/power1_cap"), b);
        }
    }
}

impl Dev {
    /// Primeira placa /sys/class/drm/cardN com o driver amdgpu.
    fn find() -> Option<Dev> {
        let mut buf = String::new();
        for card in sorted_dir("/sys/class/drm", "card") {
            if card.contains('-') || !card[4..].starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let dir = format!("/sys/class/drm/{card}/device");
            read_into(&format!("{dir}/uevent"), &mut buf);
            if !buf.lines().any(|l| l == "DRIVER=amdgpu") {
                continue;
            }
            let pdev = std::fs::canonicalize(&dir)
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let hwmon = sorted_dir(&format!("{dir}/hwmon"), "hwmon").first().map(|h| format!("{dir}/hwmon/{h}"));
            return Some(Dev {
                dir,
                hwmon,
                pdev,
                paths: Vec::new(),
                prev: None,
                spare: HashMap::new(),
                prev_t: Instant::now(),
                tick: 0,
                buf,
            });
        }
        None
    }

    fn read_clients(&mut self, out: &mut HashMap<u64, u64>) {
        let buf = &mut self.buf;
        let pdev = &self.pdev;
        self.paths.retain(|path| {
            if !read_into(path, buf) {
                return false; // fd fechado / processo saiu
            }
            if let Some((cid, ns)) = parse_fdinfo(buf, pdev) {
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
                    && let Some(fdn) = fd.file_name().to_str()
                {
                    self.paths.push(format!("/proc/{pid}/fdinfo/{fdn}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lspci_name_monta_o_nome_radeon() {
        let txt = r#"01:00.0 "VGA compatible controller" "Advanced Micro Devices, Inc. [AMD/ATI]" "Lexa PRO [Radeon 540/540X/550/550X / RX 540X/550/550X]" -rc7 "ASUSTeK""#;
        assert_eq!(lspci_name(txt).unwrap(), "AMD Radeon RX 540X/550/550X  (Lexa PRO)");
        let sem_colchetes = r#"01:00.0 "VGA" "AMD" "Navi 23""#;
        assert_eq!(lspci_name(sem_colchetes).unwrap(), "AMD Navi 23");
        assert_eq!(lspci_name(""), None);
    }

    const FDINFO: &str = "pos:\t0\nflags:\t02100002\ndrm-driver:\tamdgpu\ndrm-client-id:\t17\n\
                          drm-pdev:\t0000:01:00.0\ndrm-engine-gfx:\t1500 ns\ndrm-engine-compute:\t500 ns\n\
                          drm-engine-dma:\t9999 ns\n";

    #[test]
    fn parse_fdinfo_soma_gfx_e_compute() {
        assert_eq!(parse_fdinfo(FDINFO, "0000:01:00.0"), Some((17, 2000)));
        assert_eq!(parse_fdinfo(FDINFO, ""), Some((17, 2000)));
    }

    #[test]
    fn parse_fdinfo_ignora_outra_placa_e_outro_driver() {
        assert_eq!(parse_fdinfo(FDINFO, "0000:02:00.0"), None);
        assert_eq!(parse_fdinfo(&FDINFO.replace("amdgpu", "i915"), ""), None);
        assert_eq!(parse_fdinfo("pos:\t0\n", ""), None);
    }

    #[test]
    fn busy_ns_ignora_cliente_novo_e_contador_que_voltou() {
        let prev = HashMap::from([(1, 1000), (2, 5000)]);
        let cur = HashMap::from([(1, 1600), (2, 4000), (3, 9000)]);
        assert_eq!(busy_ns(&prev, &cur), 600);
    }
}
