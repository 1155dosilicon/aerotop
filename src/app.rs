//! Estado do programa, coleta a cada segundo, teclas e ações.
use crate::collect::gpu::Gpu;
use crate::collect::procs;
use crate::collect::procs::Procs;
use crate::collect::sys::{self, Cpu, CpuTemp, Disk, Load, Mem, Net, Zram};
use crate::ui::draw::R;
use crate::ui::theme::{GRAY, GREEN, RED};
use crate::util::{br, errno, fmt_bytes, pidfd_kill, pidfd_open, pyround};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Color;
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub const HIST: usize = 600;

/// Degraus de prioridade (valor "nice": menor = mais prioridade).
/// Para em -15 de propósito: -20 pode travar o resto do sistema.
pub const NIVEIS: [(i32, &str); 6] =
    [(19, "Baixa"), (10, "Abaixo do normal"), (0, "Normal"), (-5, "Acima do normal"), (-10, "Alta"), (-15, "Muito alta")];

pub fn nivel_nome(nice: i32) -> &'static str {
    NIVEIS.iter().find(|(n, _)| nice >= *n).map_or("Muito alta", |(_, s)| *s)
}

/// Próximo degrau de [`NIVEIS`] a partir do valor nice `atual`, andando
/// `passo` (+1 = mais prioridade). `None` se já está na ponta.
pub fn proximo_nivel(atual: i32, passo: isize) -> Option<(i32, &'static str)> {
    // degrau atual = o mais próximo do valor que o processo tem
    let idx = (0..NIVEIS.len()).min_by_key(|k| (NIVEIS[*k].0 - atual).abs())?;
    NIVEIS.get(idx.checked_add_signed(passo)?).copied()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Cpu,
    Gpu,
    Proc,
}

#[derive(Clone, Copy)]
pub enum Sev {
    Info,
    Warn,
    Error,
}

pub struct Toast {
    pub text: String,
    pub sev: Sev,
    pub until: Instant,
}

pub struct Dialog {
    pub pid: i32,
    pub name: String,
    pub yes: bool,
    /// "alça" do processo aberta quando o diálogo apareceu: o sinal vai para
    /// ele mesmo que o PID seja reaproveitado por outro. `None` em kernel
    /// antigo (< 5.3), onde só dá para conferir o início do processo de novo.
    pidfd: Option<OwnedFd>,
    start: u64,
}

pub struct OvRow {
    pub comp: &'static str,
    pub dot: Color,
    pub status: &'static str,
    pub util: String,
    pub det: String,
}

pub struct App {
    pub tab: Tab,
    pub cpu: Cpu,
    pub cpu_temp: CpuTemp,
    pub cpu_name: String,
    pub load: Load,
    pub mem: Mem,
    pub ram_info: String,
    pub zram: Zram,
    pub has_zram: bool,
    pub gpu: Gpu,
    pub gpu_use: Option<f64>,
    pub gpu_hist: VecDeque<f64>,
    pub vram_hist: VecDeque<f64>,
    net: Net,
    disk: Disk,
    pub overview: Vec<OvRow>,
    pub procs: Procs,
    procs_age: u32,
    /// índices de `procs.list` depois do filtro
    pub view: Vec<usize>,
    pub cursor: usize,
    pub scroll: usize,
    pub filter: String,
    pub filter_open: bool,
    pub filter_focus: bool,
    pub show_kthreads: bool,
    pub dialog: Option<Dialog>,
    pub toasts: Vec<Toast>,
    tx: Sender<(String, Sev)>,
    rx: Receiver<(String, Sev)>,
    pub pending: usize,
    pub me: u32,
    // áreas clicáveis, preenchidas pelo desenho
    pub tab_hits: Vec<(R, Tab)>,
    pub table_rows: R,
    pub dialog_hits: [R; 2],
}

fn st(pct: Option<f64>, busy: f64, idle: f64, labels: [&'static str; 3]) -> (Color, &'static str) {
    match pct {
        None => (GRAY, "Sem dados"),
        Some(p) if p >= busy => (RED, labels[2]),
        Some(p) => (GREEN, if p < idle { labels[0] } else { labels[1] }),
    }
}

const PADRAO: [&str; 3] = ["Ocioso", "Normal", "Ocupado"];

impl App {
    pub fn new() -> Self {
        let mut cpu = Cpu::default();
        cpu.sample();
        let mut load = Load::default();
        load.sample();
        let mut mem = Mem::default();
        mem.sample();
        let zram = Zram::new();
        let has_zram = zram.stats.is_some();
        let mut gpu = Gpu::new();
        gpu.sample();
        gpu.read_sensors(true, true);
        let (tx, rx) = channel();
        let ram_info = sys::ram_info(mem.total);
        App {
            tab: Tab::Cpu,
            cpu,
            cpu_temp: CpuTemp::new(),
            cpu_name: sys::cpu_name(),
            load,
            mem,
            ram_info,
            zram,
            has_zram,
            gpu,
            gpu_use: None,
            gpu_hist: VecDeque::with_capacity(HIST),
            vram_hist: VecDeque::with_capacity(HIST),
            net: Net::new(),
            disk: Disk::new(),
            overview: Vec::new(),
            procs: Procs::new(),
            procs_age: 0,
            view: Vec::new(),
            cursor: 0,
            scroll: 0,
            filter: String::new(),
            filter_open: false,
            filter_focus: false,
            show_kthreads: false,
            dialog: None,
            toasts: Vec::new(),
            tx,
            rx,
            pending: 0,
            // SAFETY: getuid nunca falha.
            me: unsafe { libc::getuid() },
            tab_hits: Vec::new(),
            table_rows: R::default(),
            dialog_hits: [R::default(); 2],
        }
    }

    // ───────────── coleta a cada 1 s ─────────────
    pub fn tick(&mut self) {
        self.cpu.sample();
        if self.tab != Tab::Gpu && self.cpu_temp.ok() {
            self.cpu_temp.sample();
        }
        if self.tab == Tab::Cpu {
            self.load.sample();
        }
        self.mem.sample();
        if self.has_zram {
            self.zram.sample();
        }
        // GPU e VRAM são coletadas sempre: alimentam os gráficos de histórico
        if self.gpu.ok() {
            if let Some(g) = self.gpu.sample() {
                self.gpu_use = Some(g);
                push(&mut self.gpu_hist, g);
            }
            self.gpu.read_sensors(self.tab == Tab::Gpu, self.tab == Tab::Proc);
            let s = &self.gpu.sens;
            if let (Some(vu), Some(vt)) = (s.vram_used, s.vram_total)
                && vt > 0
            {
                push(&mut self.vram_hist, 100.0 * vu as f64 / vt as f64);
            }
        }
        if self.tab == Tab::Proc {
            self.net.sample();
            self.disk.sample();
            self.build_overview();
            self.procs_age += 1;
            if self.procs_age >= 3 {
                self.refresh_procs();
            }
        }
    }

    fn build_overview(&mut self) {
        let rows = &mut self.overview;
        rows.clear();
        let cpu = self.cpu.total;
        let (dot, status) = st(Some(cpu), 70.0, 15.0, PADRAO);
        let mut det = if cpu < 15.0 {
            "Carga baixa de CPU"
        } else if cpu < 70.0 {
            "Carga moderada de CPU"
        } else {
            "CPU bem ocupada"
        }
        .to_string();
        match self.cpu_temp.pkg {
            Some(t) => det.push_str(&format!(" — {} °C", pyround(t as f64 / 1000.0))),
            None => det.push('.'),
        }
        rows.push(OvRow { comp: "CPU", dot, status, util: format!("{} %", pyround(cpu)), det });

        if let Some(n) = &self.net.best {
            let util = n.util();
            let (dot, status) = st(Some(util.unwrap_or(0.0)), 60.0, 1.0, PADRAO);
            rows.push(OvRow {
                comp: "Rede",
                dot,
                status,
                util: match util {
                    Some(u) => format!("{} %", br(u, 1)),
                    None => format!("{}/s", fmt_bytes(n.rate)),
                },
                det: format!("Adaptador mais ocupado: {} — {}/s", n.nic, fmt_bytes(n.rate)),
            });
        }

        if let Some((ops, rd, wr)) = self.disk.rate {
            let (dot, status) = if ops < 100.0 {
                (GREEN, "Ocioso")
            } else if ops < 500.0 {
                (GREEN, "Normal")
            } else {
                (RED, "Ocupado")
            };
            rows.push(OvRow {
                comp: "Disco",
                dot,
                status,
                util: format!("{} /s", pyround(ops)),
                det: format!("Leitura {}/s  ·  Escrita {}/s", fmt_bytes(rd), fmt_bytes(wr)),
            });
        }

        let (dot, status) = st(Some(self.mem.percent), 75.0, 40.0, PADRAO);
        rows.push(OvRow {
            comp: "Memória",
            dot,
            status,
            util: format!("{} %", pyround(self.mem.percent)),
            det: format!("{} MB disponíveis.", self.mem.available >> 20),
        });

        if let Some(z) = self.zram.stats.as_ref().filter(|_| self.has_zram) {
            let zp = 100.0 * z.orig as f64 / z.size as f64;
            let taxa = if z.compr > 0 && z.orig > 1 << 20 {
                format!(" (comprimido {}x)", br(z.orig as f64 / z.compr as f64, 1))
            } else {
                String::new()
            };
            let (dot, status) = st(Some(zp), 80.0, 5.0, ["Folgado", "Em uso", "Quase cheio"]);
            rows.push(OvRow {
                comp: "Zram",
                dot,
                status,
                util: format!("{} %", pyround(zp)),
                det: format!("{} MB guardados ocupando {} MB de RAM{}", z.orig >> 20, z.used >> 20, taxa),
            });
        }

        if self.gpu.ok() {
            let g = self.gpu_use;
            let mut det = self.gpu.name.split("  (").next().unwrap_or("").to_string();
            if let Some(t) = self.gpu.sens.temp {
                det.push_str(&format!(" — {} °C", pyround(t as f64 / 1000.0)));
            }
            let (dot, status) = st(g, 70.0, 10.0, PADRAO);
            rows.push(OvRow { comp: "GPU", dot, status, util: g.map(|g| format!("{} %", pyround(g))).unwrap_or_else(|| "--".into()), det });
        }
    }

    // ───────────── processos ─────────────
    pub fn refresh_procs(&mut self) {
        let keep = self.cursor_pid();
        // sem CPU% novo (leitura colada na anterior): tenta de novo no próximo segundo
        self.procs_age = if self.procs.update(self.mem.total) { 0 } else { 2 };
        self.rebuild_view(keep);
    }

    pub fn cursor_pid(&self) -> Option<i32> {
        self.view.get(self.cursor).map(|i| self.procs.list[*i].pid)
    }

    fn rebuild_view(&mut self, keep: Option<i32>) {
        let filtro = self.filter.trim().to_lowercase();
        self.view.clear();
        for (i, p) in self.procs.list.iter().enumerate() {
            if (p.pid == 2 || p.ppid == 2) && !self.show_kthreads {
                continue;
            }
            if !filtro.is_empty() && !p.name.to_lowercase().contains(&filtro) {
                continue;
            }
            self.view.push(i);
        }
        // o cursor continua no mesmo PID; a rolagem não muda
        if let Some(pos) = keep.and_then(|pid| self.view.iter().position(|i| self.procs.list[*i].pid == pid)) {
            self.cursor = pos;
        }
        self.cursor = self.cursor.min(self.view.len().saturating_sub(1));
        self.clamp_scroll();
    }

    fn visible(&self) -> usize {
        (self.table_rows.h.max(1)) as usize
    }

    fn clamp_scroll(&mut self) {
        let max = self.view.len().saturating_sub(self.visible());
        self.scroll = self.scroll.min(max);
    }

    fn move_cursor(&mut self, to: isize) {
        if self.view.is_empty() {
            return;
        }
        self.cursor = to.clamp(0, self.view.len() as isize - 1) as usize;
        let vis = self.visible();
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + vis {
            self.scroll = self.cursor + 1 - vis;
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        match tab {
            Tab::Proc => {
                // entrou na aba: atualiza tudo na hora
                self.net.sample();
                self.disk.sample();
                self.gpu.read_sensors(false, true);
                self.build_overview();
                self.refresh_procs();
                // essa leitura só serve de base (o CPU% de cada processo é a
                // diferença entre duas): a próxima vem daqui a 1 s, não 3 s
                self.procs_age = 2;
            }
            Tab::Gpu => self.gpu.read_sensors(true, true),
            Tab::Cpu => {
                self.cpu_temp.sample();
                self.load.sample();
            }
        }
    }

    // ───────────── avisos ─────────────
    pub fn notify(&mut self, text: impl Into<String>, sev: Sev) {
        self.toasts.push(Toast { text: text.into(), sev, until: Instant::now() + Duration::from_secs(3) });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn next_toast_expiry(&self) -> Option<Instant> {
        self.toasts.iter().map(|t| t.until).min()
    }

    pub fn expire_toasts(&mut self, now: Instant) -> bool {
        let n = self.toasts.len();
        self.toasts.retain(|t| t.until > now);
        n != self.toasts.len()
    }

    /// Recebe o resultado das threads do pkexec.
    pub fn drain_jobs(&mut self) -> bool {
        let mut any = false;
        while let Ok((text, sev)) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            self.notify(text, sev);
            any = true;
        }
        if any && self.tab == Tab::Proc {
            self.refresh_procs();
        }
        any
    }

    // ───────────── ações ─────────────
    fn prioridade(&mut self, passo: isize) {
        if self.tab != Tab::Proc {
            return;
        }
        let Some(&i) = self.view.get(self.cursor) else { return };
        let p = &self.procs.list[i];
        let (pid, start, name) = (p.pid, p.start, p.name.clone());
        // a lista pode ter até 3 s: se o PID já é de outro processo, a
        // prioridade iria para ele (com o pkexec, até para um processo do root)
        if !procs::is_same(pid, start) {
            self.notify(format!("{name} já tinha fechado"), Sev::Info);
            self.refresh_procs();
            return;
        }
        // SAFETY: chamadas simples da libc; errno é zerado antes porque -1 é um valor válido.
        let atual = unsafe {
            *libc::__errno_location() = 0;
            let v = libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t);
            let e = errno();
            if v == -1 && e != 0 { Err(e) } else { Ok(v) }
        };
        let atual = match atual {
            Ok(v) => v,
            Err(e) => return self.prioridade_falhou(&name, e),
        };
        let Some((novo, nome_nivel)) = proximo_nivel(atual, passo) else {
            self.notify(format!("{} já está na prioridade {}", name, nivel_nome(atual).to_lowercase()), Sev::Info);
            return;
        };
        let ok_msg = format!("{}: prioridade {}", name, nome_nivel.to_lowercase());
        // SAFETY: setpriority só recebe inteiros.
        let r = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, novo) };
        if r == 0 {
            self.notify(ok_msg, Sev::Info);
            self.refresh_procs();
            return;
        }
        match errno() {
            libc::EPERM | libc::EACCES => {}
            e => return self.prioridade_falhou(&name, e),
        }
        // O sistema negou: pede a senha pelo pkexec, numa thread, sem travar a tela
        self.notify("Subir prioridade pede a senha…", Sev::Info);
        self.pending += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let ok = std::process::Command::new("pkexec")
                .args(["renice", "-n", &novo.to_string(), "-p", &pid.to_string()])
                .stdin(std::process::Stdio::null())
                .output()
                .is_ok_and(|o| o.status.success());
            let msg = if !ok {
                ("Prioridade não mudou (senha cancelada?)".to_string(), Sev::Warn)
            } else if procs::is_same(pid, start) {
                (ok_msg, Sev::Info)
            } else {
                // a janela de senha pode ficar aberta um bom tempo e o renice não
                // aceita pidfd: se o PID trocou de dono nesse meio-tempo, ao menos avisa
                (format!("{name} fechou durante a senha; o PID {pid} agora pode ser outro processo"), Sev::Warn)
            };
            let _ = tx.send(msg);
        });
    }

    fn prioridade_falhou(&mut self, name: &str, e: i32) {
        if e == libc::ESRCH {
            self.notify(format!("{name} já tinha fechado"), Sev::Info);
            self.refresh_procs();
        } else {
            let err = std::io::Error::from_raw_os_error(e);
            self.notify(format!("Não consegui mudar a prioridade de {name}: {err}"), Sev::Error);
        }
    }

    fn encerrar(&mut self) {
        if self.tab != Tab::Proc {
            return;
        }
        let Some(&i) = self.view.get(self.cursor) else { return };
        let p = &self.procs.list[i];
        let (pid, start, name) = (p.pid, p.start, p.name.clone());
        // se falhar, ou o processo já saiu (a conferência abaixo avisa) ou o
        // kernel não tem pidfd (aí confere de novo na hora de encerrar)
        let pidfd = pidfd_open(pid).ok();
        // a lista pode ter até 3 s: confere que o PID ainda é o mesmo processo
        if !procs::is_same(pid, start) {
            self.notify(format!("{name} já tinha fechado"), Sev::Info);
            self.refresh_procs();
            return;
        }
        self.dialog = Some(Dialog { pid, name, yes: true, pidfd, start });
    }

    fn dialog_done(&mut self, ok: bool) {
        let Some(d) = self.dialog.take() else { return };
        if !ok {
            return;
        }
        let r = match &d.pidfd {
            Some(fd) => pidfd_kill(fd, libc::SIGTERM),
            None if procs::is_same(d.pid, d.start) => {
                // SAFETY: kill só recebe inteiros.
                if unsafe { libc::kill(d.pid, libc::SIGTERM) } == 0 { Ok(()) } else { Err(errno()) }
            }
            None => Err(libc::ESRCH),
        };
        match r {
            Ok(()) => self.notify(format!("{} ({}) encerrado", d.name, d.pid), Sev::Info),
            Err(libc::EPERM) => self.notify(format!("Sem permissão para encerrar {} (é do sistema/root)", d.name), Sev::Error),
            Err(libc::ESRCH) => self.notify(format!("{} já tinha fechado", d.name), Sev::Info),
            Err(e) => {
                let err = std::io::Error::from_raw_os_error(e);
                self.notify(format!("Não consegui encerrar {}: {err}", d.name), Sev::Error);
            }
        }
        self.refresh_procs();
    }

    fn close_filter(&mut self) {
        let keep = self.cursor_pid();
        self.filter.clear();
        self.filter_open = false;
        self.filter_focus = false;
        self.rebuild_view(keep);
    }

    fn nav(&mut self, code: KeyCode) -> bool {
        if self.tab != Tab::Proc {
            return false;
        }
        let cur = self.cursor as isize;
        let page = self.visible() as isize;
        match code {
            KeyCode::Up => self.move_cursor(cur - 1),
            KeyCode::Down => self.move_cursor(cur + 1),
            KeyCode::PageUp => self.move_cursor(cur - page),
            KeyCode::PageDown => self.move_cursor(cur + page),
            KeyCode::Home => self.move_cursor(0),
            KeyCode::End => self.move_cursor(isize::MAX),
            _ => return false,
        }
        true
    }

    /// Devolve `true` quando é para sair.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            return matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q'));
        }
        if let Some(d) = &mut self.dialog {
            match k.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => d.yes = !d.yes,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let ok = d.yes;
                    self.dialog_done(ok);
                }
                KeyCode::Esc => self.dialog_done(false),
                _ => {}
            }
            return false;
        }
        if self.filter_open && self.filter_focus {
            match k.code {
                KeyCode::Esc => self.close_filter(),
                KeyCode::Enter => self.filter_focus = false,
                KeyCode::Backspace => {
                    let keep = self.cursor_pid();
                    self.filter.pop();
                    self.rebuild_view(keep);
                }
                KeyCode::Char(c) => {
                    let keep = self.cursor_pid();
                    self.filter.push(c);
                    self.rebuild_view(keep);
                }
                other => {
                    self.nav(other);
                }
            }
            return false;
        }
        match k.code {
            KeyCode::Esc if self.filter_open => self.close_filter(),
            KeyCode::Char('1') => self.set_tab(Tab::Cpu),
            KeyCode::Char('2') => self.set_tab(Tab::Gpu),
            KeyCode::Char('3') => self.set_tab(Tab::Proc),
            KeyCode::Char('/') => {
                self.set_tab(Tab::Proc);
                self.filter_open = true;
                self.filter_focus = true;
            }
            KeyCode::Char('k') => self.encerrar(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.prioridade(1),
            KeyCode::Char('-') => self.prioridade(-1),
            KeyCode::Char('t') => {
                self.show_kthreads = !self.show_kthreads;
                self.notify(if self.show_kthreads { "Mostrando threads do kernel" } else { "Threads do kernel escondidas" }, Sev::Info);
                if self.tab == Tab::Proc {
                    self.refresh_procs();
                }
            }
            KeyCode::Char('q') => return true,
            other => {
                self.nav(other);
            }
        }
        false
    }

    /// Devolve `true` se algo mudou na tela.
    pub fn on_mouse(&mut self, m: MouseEvent) -> bool {
        let (x, y) = (m.column as i32, m.row as i32);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.dialog.is_some() {
                    if self.dialog_hits[0].contains(x, y) {
                        self.dialog_done(true);
                    } else if self.dialog_hits[1].contains(x, y) {
                        self.dialog_done(false);
                    }
                    return true;
                }
                if let Some((_, tab)) = self.tab_hits.iter().find(|(r, _)| r.contains(x, y)) {
                    let tab = *tab;
                    self.set_tab(tab);
                    return true;
                }
                if self.tab == Tab::Proc && self.table_rows.contains(x, y) {
                    let row = self.scroll + (y - self.table_rows.y) as usize;
                    if row < self.view.len() {
                        self.cursor = row;
                        self.filter_focus = false;
                    }
                    return true;
                }
                false
            }
            MouseEventKind::ScrollUp if self.tab == Tab::Proc && self.dialog.is_none() => {
                self.scroll = self.scroll.saturating_sub(3);
                true
            }
            MouseEventKind::ScrollDown if self.tab == Tab::Proc && self.dialog.is_none() => {
                self.scroll += 3;
                self.clamp_scroll();
                true
            }
            _ => false,
        }
    }
}

fn push(h: &mut VecDeque<f64>, v: f64) {
    if h.len() == HIST {
        h.pop_front();
    }
    h.push_back(v.clamp(0.0, 100.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nivel_nome_arredonda_para_o_degrau_de_baixo() {
        assert_eq!(nivel_nome(0), "Normal");
        assert_eq!(nivel_nome(5), "Normal");
        assert_eq!(nivel_nome(19), "Baixa");
        assert_eq!(nivel_nome(-3), "Acima do normal");
        assert_eq!(nivel_nome(-20), "Muito alta");
    }

    #[test]
    fn proximo_nivel_sobe_e_desce_um_degrau() {
        assert_eq!(proximo_nivel(0, 1), Some((-5, "Acima do normal")));
        assert_eq!(proximo_nivel(0, -1), Some((10, "Abaixo do normal")));
        // valor fora da escada: parte do degrau mais próximo
        assert_eq!(proximo_nivel(-4, 1), Some((-10, "Alta")));
        assert_eq!(proximo_nivel(18, -1), None);
        assert_eq!(proximo_nivel(-15, 1), None);
        assert_eq!(proximo_nivel(-20, 1), None);
    }
}
