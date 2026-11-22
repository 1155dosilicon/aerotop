//! AeroTop — monitor do sistema no terminal (versão em Rust).
//!
//! Abas: CPU e RAM (medidores estilo Windows 7), GPU (gráfico moderno) e
//! Processos (bolinhas de status). Teclas: 1/2/3 trocam de aba, q sai.
mod app;
mod collect;
mod ui;
mod util;

use anyhow::{Context, Result};
use app::{App, Tab};
use crossterm::event::{self, Event, KeyEventKind};
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

// Só cliques e rodinha (sem movimento do mouse, que acordaria o programa à toa).
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1000l";
/// Volta o fundo do terminal para a cor do tema dele (OSC 111).
const BG_RESET: &str = "\x1b]111\x1b\\";

/// Pinta o fundo do terminal com o do AeroTop (OSC 11): a margem do terminal
/// (o `window_padding` do kitty, a sobra de pixels) fica fora das células e
/// mostraria a cor do tema em volta da tela. Só nos terminais que sabidamente
/// voltam a cor com o OSC 111; nos outros, a cor ficaria trocada depois de sair.
/// Dentro do tmux/screen não: ele não repassa a sequência para o terminal de fora.
fn bg_osc(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    let term = env("TERM").unwrap_or_default();
    if set("TMUX") || set("STY") || term.starts_with("screen") || term.starts_with("tmux") {
        return None;
    }
    let prog = env("TERM_PROGRAM").unwrap_or_default();
    let ok = set("KITTY_WINDOW_ID")
        || set("ALACRITTY_WINDOW_ID")
        || set("WEZTERM_PANE")
        || set("GHOSTTY_RESOURCES_DIR")
        || matches!(term.as_str(), "xterm-kitty" | "alacritty" | "foot" | "foot-extra" | "xterm-ghostty" | "wezterm")
        || matches!(prog.as_str(), "WezTerm" | "ghostty");
    ok.then(|| format!("\x1b]11;#{:06x}\x1b\\", ui::theme::BG_HEX))
}

fn main() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--ram") {
        // erro numa linha só, em português ("não consegui gravar X: Permission denied")
        return Ok(ram_cmd().unwrap_or_else(|e| {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }));
    }
    if let Some(i) = args.iter().position(|a| a == "--dump") {
        dump(args.get(i + 1).map_or("120x40", String::as_str))?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut app = App::new();
    let bg = bg_osc(|k| std::env::var(k).ok());
    let reset = if bg.is_some() { BG_RESET } else { "" };
    let mut term = ratatui::init();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        print!("{MOUSE_OFF}{reset}");
        hook(info);
    }));
    // daqui até o restore nada pode sair com `?` direto: o terminal ficaria
    // em modo raw e na tela alternativa
    print!("{MOUSE_ON}{}", bg.as_deref().unwrap_or(""));
    let res = std::io::stdout().flush().map_err(anyhow::Error::from).and_then(|()| main_loop(&mut term, &mut app));
    print!("{MOUSE_OFF}{reset}");
    ratatui::restore();
    res.map(|()| ExitCode::SUCCESS)
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
fn ram_cmd() -> Result<ExitCode> {
    // o dmidecode fica em /usr/sbin, que não está no PATH de usuário comum no
    // Debian; o pkexec procura no PATH de quem chamou, então vai o caminho inteiro
    let dmi = ["/usr/sbin/dmidecode", "/sbin/dmidecode", "/usr/bin/dmidecode", "/bin/dmidecode"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("dmidecode");
    // SAFETY: geteuid não tem pré-condições.
    let root = unsafe { libc::geteuid() } == 0;
    // decide antes de pedir a senha: se não der para saber onde gravar, nem pede
    let dest = cache_dest(root, |v| std::env::var(v).ok(), user_home)?;
    let out = if root {
        std::process::Command::new(dmi).args(["-t", "memory"]).output()
    } else {
        std::process::Command::new("pkexec").args([dmi, "-t", "memory"]).output()
    };
    let falha = |msg: String| {
        eprintln!("{msg}");
        Ok(ExitCode::FAILURE)
    };
    let o = match out {
        Ok(o) if o.status.success() => o,
        // 126/127: o pkexec não autorizou (janela fechada ou senha errada)
        Ok(o) if !root && matches!(o.status.code(), Some(126 | 127)) => {
            return falha("Não consegui ler: a senha foi cancelada ou recusada.".into());
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            return falha(format!("Não consegui ler a memória com o {dmi}: {}", err.trim()));
        }
        Err(e) => return falha(format!("Não consegui rodar o {}: {e}", if root { dmi } else { "pkexec" })),
    };
    // a parte que precisa de root acabou: volta a ser o usuário antes de
    // mexer na pasta dele (assim nada é criado como root nem segue atalhos como root)
    let home = match dest {
        CacheDest::Caller { uid, gid, home } => {
            drop_to(uid, gid).context("não consegui voltar a ser o usuário que chamou")?;
            home
        }
        CacheDest::Me { home } => home,
    };
    let dir = format!("{home}/.cache/aerotop");
    std::fs::create_dir_all(&dir).with_context(|| format!("não consegui criar {dir}"))?;
    let file = format!("{dir}/dmidecode-memory.txt");
    std::fs::write(&file, &o.stdout).with_context(|| format!("não consegui gravar {file}"))?;
    let mut mem = collect::sys::Mem::default();
    mem.sample();
    println!("RAM: {}", collect::sys::ram_info_from(&home, mem.total));
    Ok(ExitCode::SUCCESS)
}

/// Em nome de quem e em que pasta pessoal o `--ram` grava o cache.
#[derive(Debug, PartialEq, Eq)]
enum CacheDest {
    /// root a pedido de outro usuário (pkexec/sudo): volta a ser ele antes de gravar
    Caller { uid: u32, gid: u32, home: String },
    /// o próprio usuário (ou root de verdade, sem ninguém por trás)
    Me { home: String },
}

/// Decide o [`CacheDest`]. `var` lê variáveis de ambiente e `lookup` acha
/// (pasta pessoal, grupo) de um uid; ficam de fora para os testes.
fn cache_dest(root: bool, var: impl Fn(&str) -> Option<String>, lookup: impl Fn(u32) -> Option<(String, u32)>) -> Result<CacheDest> {
    // rodando como root (pkexec/sudo na frente): o cache vai para quem chamou
    let caller = if root { ["PKEXEC_UID", "SUDO_UID"].into_iter().find_map(|v| var(v)?.parse::<u32>().ok()) } else { None };
    if let Some(uid) = caller {
        // sem achar o usuário não dá para largar o root: gravar como root no
        // $HOME (que com sudo pode ser a pasta dele) é justamente o que se evita
        let (home, gid) = lookup(uid).with_context(|| format!("não achei a pasta pessoal do usuário {uid}"))?;
        return Ok(CacheDest::Caller { uid, gid, home });
    }
    match var("HOME").filter(|h| !h.is_empty()) {
        Some(home) => Ok(CacheDest::Me { home }),
        None => anyhow::bail!("não sei onde gravar o cache: HOME não está definido"),
    }
}

/// Deixa de ser root de vez e vira o usuário `uid` (grupo principal `gid`).
fn drop_to(uid: u32, gid: u32) -> std::io::Result<()> {
    // SAFETY: chamadas simples da libc. A ordem importa: grupos e gid antes do
    // uid, porque depois do setuid não há mais permissão para trocá-los.
    let ok = unsafe { libc::setgroups(1, &gid) == 0 && libc::setgid(gid) == 0 && libc::setuid(uid) == 0 };
    if ok { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// (pasta pessoal, grupo) de um usuário, pelo /etc/passwd
fn user_home(uid: u32) -> Option<(String, u32)> {
    // SAFETY: getpwuid devolve ponteiro para área estática; só a thread principal usa.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some((std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned(), (*pw).pw_gid))
    }
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
            // saída fechada antes do fim (ex.: `aerotop --dump | head`): só para
            if writeln!(std::io::stdout(), "{}", line.trim_end()).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    fn eli(uid: u32) -> Option<(String, u32)> {
        (uid == 1000).then(|| ("/home/eli".to_string(), 1000))
    }

    #[test]
    fn bg_osc_so_nos_terminais_que_voltam_a_cor() {
        let osc = Some("\x1b]11;#0a0f1a\x1b\\".to_string());
        assert_eq!(bg_osc(env(&[("TERM", "xterm-kitty"), ("KITTY_WINDOW_ID", "1")])), osc);
        assert_eq!(bg_osc(env(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")])), osc);
        // terminal desconhecido: a cor poderia ficar trocada depois de sair
        assert_eq!(bg_osc(env(&[("TERM", "xterm-256color")])), None);
        assert_eq!(bg_osc(env(&[])), None);
        // kitty, mas dentro do tmux (as variáveis do kitty passam para dentro)
        assert_eq!(bg_osc(env(&[("TERM", "tmux-256color"), ("KITTY_WINDOW_ID", "1"), ("TMUX", "/tmp/tmux-1000/default,1,0")])), None);
        assert_eq!(bg_osc(env(&[("TERM", "screen"), ("KITTY_WINDOW_ID", "1")])), None);
    }

    #[test]
    fn cache_dest_como_root_vai_para_quem_chamou() {
        let vars = [("PKEXEC_UID", "1000"), ("HOME", "/root")];
        assert_eq!(cache_dest(true, env(&vars), eli).unwrap(), CacheDest::Caller { uid: 1000, gid: 1000, home: "/home/eli".into() });
        let sudo = [("SUDO_UID", "1000"), ("HOME", "/home/eli")];
        assert_eq!(cache_dest(true, env(&sudo), eli).unwrap(), CacheDest::Caller { uid: 1000, gid: 1000, home: "/home/eli".into() });
    }

    #[test]
    fn cache_dest_nao_grava_como_root_se_nao_acha_o_usuario() {
        // antes caía no $HOME ainda como root (com sudo, a pasta do próprio usuário)
        let vars = [("SUDO_UID", "1234"), ("HOME", "/home/eli")];
        let err = cache_dest(true, env(&vars), eli).unwrap_err();
        assert_eq!(err.to_string(), "não achei a pasta pessoal do usuário 1234");
    }

    #[test]
    fn cache_dest_sem_root_usa_o_home_e_ignora_as_variaveis_do_sudo() {
        let vars = [("SUDO_UID", "1234"), ("HOME", "/home/eli")];
        assert_eq!(cache_dest(false, env(&vars), eli).unwrap(), CacheDest::Me { home: "/home/eli".into() });
        // root de verdade, sem pkexec/sudo
        assert_eq!(cache_dest(true, env(&[("HOME", "/root")]), eli).unwrap(), CacheDest::Me { home: "/root".into() });
    }

    #[test]
    fn cache_dest_sem_home_da_erro_em_vez_de_gravar_na_raiz() {
        assert!(cache_dest(false, env(&[]), eli).is_err());
        assert!(cache_dest(false, env(&[("HOME", "")]), eli).is_err());
    }
}
