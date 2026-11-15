//! AeroTop — monitor do sistema no terminal (versão em Rust).
//!
//! Abas: CPU e RAM (medidores estilo Windows 7), GPU (gráfico moderno) e
//! Processos (bolinhas de status). Teclas: 1/2/3 trocam de aba, q sai.
mod app;
mod collect;
mod ui;
mod util;

use anyhow::Result;
use app::{App, Tab};
use crossterm::event::{self, Event, KeyEventKind};
use std::io::Write;
use std::time::{Duration, Instant};

// Só cliques e rodinha (sem movimento do mouse, que acordaria o programa à toa).
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1000l";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--ram") {
        return ram_cmd();
    }
    if let Some(i) = args.iter().position(|a| a == "--dump") {
        return dump(args.get(i + 1).map(String::as_str).unwrap_or("120x40"));
    }
    let mut app = App::new();
    let mut term = ratatui::init();
    print!("{MOUSE_ON}");
    std::io::stdout().flush()?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        print!("{MOUSE_OFF}");
        hook(info);
    }));
    let res = main_loop(&mut term, &mut app);
    print!("{MOUSE_OFF}");
    ratatui::restore();
    res
}

fn main_loop(term: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let second = Duration::from_secs(1);
    let mut next_tick = Instant::now() + Duration::from_millis(250);
    let mut dirty = true;
    loop {
        // Só desenha quando algo mudou: tick, tecla, clique ou resize.
        if dirty {
            term.draw(|f| ui::view::draw(f, app))?;
            dirty = false;
        }
        let now = Instant::now();
        let mut deadline = next_tick;
        if let Some(t) = app.next_toast_expiry() {
            deadline = deadline.min(t);
        }
        if app.pending > 0 {
            deadline = deadline.min(now + Duration::from_millis(150));
        }
        if event::poll(deadline.saturating_duration_since(now))? {
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if app.on_key(k) {
                            return Ok(());
                        }
                        dirty = true;
                    }
                    Event::Mouse(m) => dirty |= app.on_mouse(m),
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        let now = Instant::now();
        if now >= next_tick {
            app.tick();
            dirty = true;
            next_tick += second;
            if next_tick <= now {
                next_tick = now + second; // ficou para trás (suspensão, etc.)
            }
        }
        dirty |= app.expire_toasts(now);
        if app.pending > 0 {
            dirty |= app.drain_jobs();
        }
    }
}

/// `aerotop --ram`: guarda a saída do dmidecode (precisa de senha) em cache.
fn ram_cmd() -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{home}/.cache/aerotop");
    std::fs::create_dir_all(&dir)?;
    let out = std::process::Command::new("pkexec").args(["dmidecode", "-t", "memory"]).output();
    match out {
        Ok(o) if o.status.success() => {
            std::fs::write(format!("{dir}/dmidecode-memory.txt"), &o.stdout)?;
            let mut mem = collect::sys::Mem::default();
            mem.sample();
            println!("RAM: {}", collect::sys::ram_info(mem.total));
        }
        _ => println!("Não consegui ler (pkexec cancelado?)"),
    }
    Ok(())
}

/// `--dump [LxA]`: desenha as três abas como texto (para conferência e testes).
fn dump(size: &str) -> Result<()> {
    use ratatui::{Terminal, backend::TestBackend};
    let (w, h) = size.split_once('x').unwrap_or(("120", "40"));
    let (w, h): (u16, u16) = (w.parse().unwrap_or(120), h.parse().unwrap_or(40));
    let mut app = App::new();
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    for tab in [Tab::Cpu, Tab::Gpu, Tab::Proc] {
        app.set_tab(tab);
        term.draw(|f| ui::view::draw(f, &mut app))?; // 1º desenho: calcula a área da tabela
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(1000));
            app.tick();
        }
        term.draw(|f| ui::view::draw(f, &mut app))?;
        let buf = term.backend().buffer();
        for y in 0..h {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
            println!("{}", line.trim_end());
        }
    }
    Ok(())
}
