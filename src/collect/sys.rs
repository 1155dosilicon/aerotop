//! CPU, memória, zram, rede e disco — lidos direto de /proc e /sys.
use crate::util::{read_int, read_into, round1};
use std::collections::HashMap;
use std::time::Instant;

// ───────────────────────────── CPU ─────────────────────────────
#[derive(Default)]
pub struct Cpu {
    prev: Vec<(u64, u64)>, // (ocupado, total) por núcleo
    pub per: Vec<f64>,
    /// número de cada CPU lógica de `per` (as desligadas não aparecem no /proc/stat)
    pub ids: Vec<usize>,
    pub total: f64,
    buf: String,
}

impl Cpu {
    pub fn sample(&mut self) {
        let mut buf = std::mem::take(&mut self.buf);
        if read_into("/proc/stat", &mut buf) {
            self.update(&buf);
        }
        self.buf = buf;
    }

    /// Atualiza o uso por núcleo com um novo conteúdo de /proc/stat.
    fn update(&mut self, stat: &str) {
        let mut i = 0;
        for line in stat.lines() {
            let Some(rest) = line.strip_prefix("cpu") else { break };
            if !rest.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                continue; // linha "cpu " agregada
            }
            let mut toks = rest.split_ascii_whitespace();
            let id: usize = toks.next().and_then(|v| v.parse().ok()).unwrap_or(i);
            let mut f = [0u64; 8];
            for (k, tok) in toks.take(8).enumerate() {
                f[k] = tok.parse().unwrap_or(0);
            }
            let total: u64 = f.iter().sum();
            let busy = total - f[3] - f[4]; // tira idle e iowait
            if i >= self.prev.len() {
                self.prev.push((busy, total));
                self.per.push(0.0);
                self.ids.push(id);
            } else {
                self.ids[i] = id;
                let (pb, pt) = self.prev[i];
                let dt = total.saturating_sub(pt);
                self.per[i] = if dt == 0 {
                    0.0
                } else {
                    round1((100.0 * busy.saturating_sub(pb) as f64 / dt as f64).clamp(0.0, 100.0))
                };
                self.prev[i] = (busy, total);
            }
            i += 1;
        }
        self.prev.truncate(i);
        self.per.truncate(i);
        self.ids.truncate(i);
        self.total = if self.per.is_empty() {
            0.0
        } else {
            self.per.iter().sum::<f64>() / self.per.len() as f64
        };
    }
}

// ───────────────────────────── temperatura da CPU ─────────────────────────────
/// Sensor de temperatura da CPU, achado no /sys/class/hwmon:
/// `coretemp` (Intel, com pacote e núcleos), `k10temp`/`zenpower` (AMD),
/// `cpu_thermal` (ARM) ou, por último, a thermal zone `x86_pkg_temp`.
#[derive(Default)]
pub struct CpuTemp {
    pkg_paths: Vec<String>,
    core_paths: Vec<String>,
    /// por número de CPU lógica: índice em `core_paths`
    cpu_core: Vec<Option<usize>>,
    /// limite "quente" do sensor, em m°C (temp*_max), se houver
    pub high: Option<i64>,
    /// temperatura do pacote (a maior, se houver mais de um soquete), em m°C
    pub pkg: Option<i64>,
    core_vals: Vec<Option<i64>>,
    buf: String,
}

impl CpuTemp {
    pub fn new() -> Self {
        let mut t = CpuTemp::default();
        let mut hw: Vec<(String, String)> = std::fs::read_dir("/sys/class/hwmon")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path().to_string_lossy().into_owned();
                let name = std::fs::read_to_string(format!("{p}/name")).ok()?;
                Some((name.trim().to_string(), p))
            })
            .collect();
        hw.sort_by(|a, b| a.1.cmp(&b.1));

        // Intel coretemp: um hwmon por soquete, com "Package id N" e "Core K"
        let mut cores: HashMap<(i64, i64), usize> = HashMap::new();
        for (_, h) in hw.iter().filter(|(n, _)| n == "coretemp") {
            let mut pkg_id = None;
            let mut local: Vec<(i64, String)> = Vec::new();
            for (label, input) in temp_inputs(h) {
                if let Some(v) = label.strip_prefix("Package id ") {
                    pkg_id = v.trim().parse().ok();
                    t.pkg_paths.push(input.clone());
                    t.read_high(&input);
                } else if let Some(v) = label.strip_prefix("Core ")
                    && let Ok(k) = v.trim().parse() {
                        local.push((k, input));
                    }
            }
            for (k, input) in local {
                cores.insert((pkg_id.unwrap_or(0), k), t.core_paths.len());
                t.core_paths.push(input);
            }
        }
        if !cores.is_empty() {
            let mut b = String::new();
            for id in 0..1024usize {
                let base = format!("/sys/devices/system/cpu/cpu{id}/topology");
                let Some(core) = read_int(&format!("{base}/core_id"), &mut b) else {
                    if id > 0 && !std::path::Path::new(&format!("/sys/devices/system/cpu/cpu{id}")).exists() {
                        break;
                    }
                    t.cpu_core.push(None);
                    continue;
                };
                let pkg = read_int(&format!("{base}/physical_package_id"), &mut b).unwrap_or(0);
                t.cpu_core.push(cores.get(&(pkg, core)).copied());
            }
        }

        // AMD e outros: só a temperatura geral
        if t.pkg_paths.is_empty() {
            for want in ["k10temp", "zenpower", "cpu_thermal", "soc_thermal"] {
                let Some((_, h)) = hw.iter().find(|(n, _)| n == want) else { continue };
                let inputs = temp_inputs(h);
                // k10temp: Tdie é a real; Tctl pode ter deslocamento em alguns Ryzen
                let pick = ["Tdie", "Tctl"]
                    .iter()
                    .find_map(|l| inputs.iter().find(|(lab, _)| lab == l))
                    .or_else(|| inputs.first());
                if let Some((_, input)) = pick {
                    t.pkg_paths.push(input.clone());
                    t.read_high(input);
                    break;
                }
            }
        }
        if t.pkg_paths.is_empty() {
            for e in std::fs::read_dir("/sys/class/thermal").into_iter().flatten().flatten() {
                let p = e.path().to_string_lossy().into_owned();
                let ty = std::fs::read_to_string(format!("{p}/type")).unwrap_or_default();
                if ty.trim() == "x86_pkg_temp" {
                    t.pkg_paths.push(format!("{p}/temp"));
                    break;
                }
            }
        }
        t.core_vals = vec![None; t.core_paths.len()];
        t.sample();
        t
    }

    fn read_high(&mut self, input: &str) {
        if self.high.is_none() {
            let max = input.replace("_input", "_max");
            self.high = read_int(&max, &mut self.buf).filter(|v| *v > 0);
        }
    }

    pub fn ok(&self) -> bool {
        !self.pkg_paths.is_empty() || !self.core_paths.is_empty()
    }

    pub fn sample(&mut self) {
        let b = &mut self.buf;
        for (v, p) in self.core_vals.iter_mut().zip(&self.core_paths) {
            *v = read_int(p, b);
        }
        self.pkg = self.pkg_paths.iter().filter_map(|p| read_int(p, b)).max();
        if self.pkg.is_none() {
            self.pkg = self.core_vals.iter().flatten().copied().max();
        }
    }

    /// temperatura do núcleo físico onde está a CPU lógica `id`, em m°C
    pub fn of_cpu(&self, id: usize) -> Option<i64> {
        self.cpu_core.get(id).copied().flatten().and_then(|k| self.core_vals[k])
    }
}

/// (rótulo, caminho do temp*_input) de um hwmon, em ordem numérica
fn temp_inputs(h: &str) -> Vec<(String, String)> {
    let mut v: Vec<(u32, String, String)> = std::fs::read_dir(h)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().into_string().ok()?;
            let n: u32 = f.strip_prefix("temp")?.strip_suffix("_input")?.parse().ok()?;
            let label = std::fs::read_to_string(format!("{h}/temp{n}_label")).unwrap_or_default();
            Some((n, label.trim().to_string(), format!("{h}/{f}")))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, l, p)| (l, p)).collect()
}

pub fn cpu_name() -> String {
    let mut buf = String::new();
    read_into("/proc/cpuinfo", &mut buf);
    let name = buf
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim())
        .unwrap_or("CPU desconhecida");
    name.replace("(R)", "").replace("(TM)", "").replace("  ", " ").trim().to_string()
}

// ───────────────────────────── memória ─────────────────────────────
#[derive(Default)]
pub struct Mem {
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub cache: u64, // buffers + cached (com SReclaimable), como o psutil
    pub percent: f64,
    pub swap_total: u64,
    pub swap_used: u64,
    buf: String,
}

impl Mem {
    pub fn sample(&mut self) {
        let mut buf = std::mem::take(&mut self.buf);
        if read_into("/proc/meminfo", &mut buf) {
            self.update(&buf);
        }
        self.buf = buf;
    }

    fn update(&mut self, meminfo: &str) {
        let (mut total, mut avail, mut buffers, mut cached, mut srecl, mut st, mut sf) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        for line in meminfo.lines() {
            let Some((key, rest)) = line.split_once(':') else { continue };
            let slot = match key {
                "MemTotal" => &mut total,
                "MemAvailable" => &mut avail,
                "Buffers" => &mut buffers,
                "Cached" => &mut cached,
                "SReclaimable" => &mut srecl,
                "SwapTotal" => &mut st,
                "SwapFree" => &mut sf,
                _ => continue,
            };
            *slot = rest.split_ascii_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0) * 1024;
        }
        self.total = total;
        self.available = avail;
        self.cache = buffers + cached + srecl;
        // psutil 7: "used" = total - disponível
        self.used = total.saturating_sub(avail);
        self.percent = if total > 0 { round1(100.0 * self.used as f64 / total as f64) } else { 0.0 };
        self.swap_total = st;
        self.swap_used = st.saturating_sub(sf);
    }
}

/// "11891 MB  ·  DDR3 1333 MT/s  ·  4 pentes" — velocidade vem do cache do dmidecode.
pub fn ram_info(total_bytes: u64) -> String {
    ram_info_from(&std::env::var("HOME").unwrap_or_default(), total_bytes)
}

pub fn ram_info_from(home: &str, total_bytes: u64) -> String {
    let mut txt = String::new();
    read_into(&format!("{home}/.cache/aerotop/dmidecode-memory.txt"), &mut txt);
    ram_info_text(&txt, total_bytes)
}

/// Resumo da RAM a partir da saída do `dmidecode -t memory` (pode ser vazia).
fn ram_info_text(txt: &str, total_bytes: u64) -> String {
    let mut parts = vec![format!("{} MB", total_bytes >> 20)];

    fn speed_of(v: &str) -> Option<String> {
        let v = v.trim_start();
        let nd = v.bytes().take_while(u8::is_ascii_digit).count();
        if nd == 0 {
            return None;
        }
        let unit = v[nd..].trim_start();
        let unit = if unit.starts_with("MT/s") {
            "MT/s"
        } else if unit.starts_with("MHz") {
            "MHz"
        } else {
            return None;
        };
        Some(format!("{} {}", &v[..nd], unit))
    }

    let (mut tipo, mut conf, mut speed, mut sticks) = (None, None, None, 0);
    for line in txt.lines() {
        let l = line.trim_start();
        if let Some(v) = l.strip_prefix("Type:") {
            let t = v.split_whitespace().next().unwrap_or("");
            if tipo.is_none() && t.starts_with("DDR") {
                tipo = Some(t.to_string());
            }
        } else if let Some(v) = l.strip_prefix("Configured Memory Speed:") {
            if conf.is_none() {
                conf = speed_of(v);
            }
        } else if let Some(v) = l.strip_prefix("Speed:") {
            if speed.is_none() {
                speed = speed_of(v);
            }
        } else if let Some(v) = l.strip_prefix("Size:") {
            let v = v.trim_start();
            let nd = v.bytes().take_while(u8::is_ascii_digit).count();
            let u = v[nd..].trim_start();
            if nd > 0 && (u.starts_with("GB") || u.starts_with("MB")) {
                sticks += 1;
            }
        }
    }
    let speed = conf.or(speed);
    if tipo.is_some() || speed.is_some() {
        let v: Vec<String> = [tipo, speed].into_iter().flatten().collect();
        parts.push(v.join(" "));
    } else {
        parts.push("velocidade: rode  aerotop --ram".to_string());
    }
    if sticks > 0 {
        parts.push(format!("{} pente{}", sticks, if sticks > 1 { "s" } else { "" }));
    }
    parts.join("  ·  ")
}

// ───────────────────────────── zram ─────────────────────────────
#[derive(Clone, Default)]
pub struct ZramStats {
    pub size: u64,
    pub orig: u64,
    pub compr: u64,
    pub used: u64,
}

pub struct Zram {
    devs: Vec<(String, String)>, // (disksize, mm_stat)
    pub alg: String,
    pub stats: Option<ZramStats>,
    buf: String,
}

impl Zram {
    pub fn new() -> Self {
        let mut names: Vec<String> = std::fs::read_dir("/sys/block")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("zram"))
            .collect();
        names.sort();
        let mut z = Zram {
            devs: names
                .iter()
                .map(|n| (format!("/sys/block/{n}/disksize"), format!("/sys/block/{n}/mm_stat")))
                .collect(),
            alg: "?".into(),
            stats: None,
            buf: String::new(),
        };
        z.sample();
        // algoritmo do primeiro dispositivo ativo: o que está entre colchetes
        for n in &names {
            let mut b = String::new();
            if read_int(&format!("/sys/block/{n}/disksize"), &mut b).unwrap_or(0) == 0 {
                continue;
            }
            read_into(&format!("/sys/block/{n}/comp_algorithm"), &mut b);
            if let Some(alg) = selected_alg(&b) {
                z.alg = alg.to_string();
            }
            break;
        }
        z
    }

    pub fn sample(&mut self) {
        let mut out: Option<ZramStats> = None;
        for (psize, pmm) in &self.devs {
            let size = read_int(psize, &mut self.buf).unwrap_or(0);
            if size <= 0 || !read_into(pmm, &mut self.buf) {
                continue;
            }
            let mut it = self.buf.split_ascii_whitespace().map(|v| v.parse::<u64>().unwrap_or(0));
            let (Some(a), Some(b), Some(c)) = (it.next(), it.next(), it.next()) else { continue };
            let o = out.get_or_insert_with(ZramStats::default);
            o.size += size as u64;
            o.orig += a;
            o.compr += b;
            o.used += c;
        }
        self.stats = out;
    }
}

/// Em `lzo lzo-rle [zstd] lz4`, o algoritmo em uso é o que está entre colchetes.
fn selected_alg(txt: &str) -> Option<&str> {
    let (a, e) = (txt.find('[')?, txt.find(']')?);
    (e > a).then(|| &txt[a + 1..e])
}

// ───────────────────────────── rede ─────────────────────────────
/// Adaptador de rede mais ocupado no último intervalo.
pub struct NetBest {
    pub nic: String,
    /// bytes/s, somando entrada e saída
    pub rate: f64,
    /// bytes/s da direção mais cheia: o link é full duplex (a velocidade vale
    /// para cada lado), então é ela que se compara com a velocidade
    pub peak: f64,
    /// velocidade do link em Mbit/s (0 = desconhecida, como no Wi-Fi)
    pub speed: i64,
}

impl NetBest {
    /// Uso do link em %, quando a velocidade é conhecida.
    pub fn util(&self) -> Option<f64> {
        (self.speed > 0).then(|| (100.0 * self.peak * 8.0 / (self.speed as f64 * 1e6)).min(100.0))
    }
}

pub struct Net {
    prev: HashMap<String, (u64, u64)>,
    prev_t: Instant,
    pub best: Option<NetBest>,
    buf: String,
}

/// Uma linha de /proc/net/dev: (adaptador, bytes recebidos, bytes enviados).
fn netdev_line(line: &str) -> Option<(&str, u64, u64)> {
    let (nic, rest) = line.split_once(':')?;
    let mut it = rest.split_ascii_whitespace();
    let rx = it.next()?.parse().ok()?;
    let tx = it.nth(7)?.parse().ok()?;
    Some((nic.trim(), rx, tx))
}

impl Net {
    pub fn new() -> Self {
        let mut n = Net { prev: HashMap::new(), prev_t: Instant::now(), best: None, buf: String::new() };
        n.sample();
        n.best = None;
        n
    }

    pub fn sample(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_t).as_secs_f64().max(0.001);
        self.prev_t = now;
        if !read_into("/proc/net/dev", &mut self.buf) {
            return;
        }
        // (adaptador, total B/s, direção mais cheia B/s)
        let mut best: Option<(&str, f64, f64)> = None;
        for line in self.buf.lines().skip(2) {
            let Some((nic, rx, tx)) = netdev_line(line) else { continue };
            let old = match self.prev.get_mut(nic) {
                Some(p) => Some(std::mem::replace(p, (rx, tx))),
                None => {
                    self.prev.insert(nic.to_string(), (rx, tx));
                    None
                }
            };
            if nic == "lo" {
                continue;
            }
            if let Some((orx, otx)) = old {
                let (r, t) = (rx.saturating_sub(orx) as f64 / dt, tx.saturating_sub(otx) as f64 / dt);
                if best.is_none_or(|b| r + t > b.1) {
                    best = Some((nic, r + t, r.max(t)));
                }
            }
        }
        let best = best.map(|(n, rate, peak)| (n.to_string(), rate, peak));
        self.best = best.map(|(nic, rate, peak)| {
            let speed = read_int(&format!("/sys/class/net/{nic}/speed"), &mut self.buf).unwrap_or(0).max(0);
            NetBest { nic, rate, peak, speed }
        });
    }
}

// ───────────────────────────── disco ─────────────────────────────
pub struct Disk {
    prev: Option<[u64; 4]>, // leituras, escritas, bytes lidos, bytes escritos
    prev_t: Instant,
    real: HashMap<String, bool>,
    /// (operações/s, leitura B/s, escrita B/s)
    pub rate: Option<(f64, f64, f64)>,
    buf: String,
}

/// Uma linha de /proc/diskstats: (disco, [leituras, escritas, setores lidos,
/// setores escritos]). O setor ali é sempre de 512 bytes.
fn diskstats_line(line: &str) -> Option<(&str, [u64; 4])> {
    let mut it = line.split_ascii_whitespace();
    let name = it.nth(2)?;
    let mut num = || it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let reads = num();
    num(); // leituras agrupadas
    let rsec = num();
    num(); // tempo lendo
    let writes = num();
    num(); // escritas agrupadas
    let wsec = num();
    Some((name, [reads, writes, rsec, wsec]))
}

impl Disk {
    pub fn new() -> Self {
        let mut d = Disk { prev: None, prev_t: Instant::now(), real: HashMap::new(), rate: None, buf: String::new() };
        d.sample();
        d
    }

    pub fn sample(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_t).as_secs_f64().max(0.001);
        self.prev_t = now;
        if !read_into("/proc/diskstats", &mut self.buf) {
            return;
        }
        let mut cur = [0u64; 4];
        for line in self.buf.lines() {
            let Some((name, [reads, writes, rsec, wsec])) = diskstats_line(line) else { continue };
            // só discos inteiros de verdade (sem partições, loop, ram, zram)
            let real = match self.real.get(name) {
                Some(r) => *r,
                None => {
                    let r = std::path::Path::new(&format!("/sys/block/{name}/device")).exists();
                    self.real.insert(name.to_string(), r);
                    r
                }
            };
            if !real {
                continue;
            }
            cur[0] += reads;
            cur[1] += writes;
            cur[2] += rsec * 512;
            cur[3] += wsec * 512;
        }
        self.rate = self.prev.map(|p| {
            let d = |i: usize| cur[i].saturating_sub(p[i]) as f64 / dt;
            (d(0) + d(1), d(2), d(3))
        });
        self.prev = Some(cur);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_calcula_uso_por_nucleo_entre_duas_leituras() {
        let mut cpu = Cpu::default();
        cpu.update("cpu  100 0 100 1800 0 0 0 0 0 0\ncpu0 100 0 100 800 0 0 0 0 0 0\ncpu1 0 0 0 1000 0 0 0 0 0 0\nintr 1 2\n");
        assert_eq!(cpu.per, [0.0, 0.0]);
        // cpu0: +100 ocupado em +200 no total = 50%; cpu1 só ficou parada
        cpu.update("cpu  150 0 150 2000 0 0 0 0 0 0\ncpu0 150 0 150 900 0 0 0 0 0 0\ncpu1 0 0 0 1100 0 0 0 0 0 0\n");
        assert_eq!(cpu.per, [50.0, 0.0]);
        assert_eq!(cpu.total, 25.0);
        assert_eq!(cpu.ids, [0, 1]);
    }

    #[test]
    fn cpu_iowait_conta_como_parado_e_cpu_desligada_some() {
        let mut cpu = Cpu::default();
        cpu.update("cpu0 0 0 0 0 0 0 0 0\ncpu2 0 0 0 0 0 0 0 0\n");
        cpu.update("cpu0 10 0 0 0 90 0 0 0\ncpu2 0 0 0 100 0 0 0 0\n");
        assert_eq!(cpu.per, [10.0, 0.0]);
        assert_eq!(cpu.ids, [0, 2]);
    }

    #[test]
    fn mem_le_meminfo_como_o_psutil() {
        let mut m = Mem::default();
        m.update(
            "MemTotal:        8000 kB\nMemFree:         1000 kB\nMemAvailable:    6000 kB\nBuffers:          100 kB\n\
             Cached:          2000 kB\nSwapCached:         0 kB\nSReclaimable:     300 kB\nSwapTotal:       4000 kB\n\
             SwapFree:        3000 kB\n",
        );
        assert_eq!((m.total, m.available, m.used), (8000 * 1024, 6000 * 1024, 2000 * 1024));
        assert_eq!(m.cache, 2400 * 1024);
        assert_eq!(m.percent, 25.0);
        assert_eq!((m.swap_total, m.swap_used), (4000 * 1024, 1000 * 1024));
    }

    const DMI: &str = "Memory Device\n\tSize: 4 GB\n\tType: DDR3\n\tSpeed: 1600 MT/s\n\
                       \tConfigured Memory Speed: 1333 MT/s\nMemory Device\n\tSize: No Module Installed\n\
                       \tType: Unknown\n\tSpeed: Unknown\nMemory Device\n\tSize: 4096 MB\n\tType: DDR3\n";

    #[test]
    fn ram_info_prefere_a_velocidade_configurada_e_conta_pentes() {
        assert_eq!(ram_info_text(DMI, 8 << 30), "8192 MB  ·  DDR3 1333 MT/s  ·  2 pentes");
        assert_eq!(ram_info_text("\tSize: 8 GB\n\tType: DDR4\n\tSpeed: 2400 MHz\n", 8 << 30), "8192 MB  ·  DDR4 2400 MHz  ·  1 pente");
    }

    #[test]
    fn ram_info_sem_cache_ensina_o_comando() {
        assert_eq!(ram_info_text("", 8 << 30), "8192 MB  ·  velocidade: rode  aerotop --ram");
    }

    #[test]
    fn zram_pega_o_algoritmo_entre_colchetes() {
        assert_eq!(selected_alg("lzo lzo-rle [zstd] lz4\n"), Some("zstd"));
        assert_eq!(selected_alg("lzo zstd\n"), None);
    }

    #[test]
    fn netdev_le_bytes_recebidos_e_enviados() {
        let line = "  enp3s0: 123456  100  0 0 0 0 0 0 654321 90 0 0 0 0 0 0";
        assert_eq!(netdev_line(line), Some(("enp3s0", 123456, 654321)));
        assert_eq!(netdev_line(" face |bytes    packets"), None);
    }

    #[test]
    fn uso_da_rede_compara_a_direcao_mais_cheia_com_o_link() {
        // 100 Mbit/s de download e 100 Mbit/s de upload num link de 100 Mbit/s: cheio, não 200%
        let b = NetBest { nic: "eth0".into(), rate: 25e6, peak: 12.5e6, speed: 100 };
        assert_eq!(b.util(), Some(100.0));
        let wifi = NetBest { speed: 0, ..b };
        assert_eq!(wifi.util(), None);
    }

    #[test]
    fn diskstats_le_operacoes_e_setores() {
        let line = "   8       0 sda 1000 50 20000 300 400 60 8000 500 0 700 800 0 0 0 0";
        assert_eq!(diskstats_line(line), Some(("sda", [1000, 400, 20000, 8000])));
        assert_eq!(diskstats_line("8 0"), None);
    }
}
