//! Desenho da tela: barra de abas, as três abas, rodapé, diálogo e avisos.
use crate::app::{App, Sev, Tab, nivel_nome};
use crate::collect::procs::Rank;
use crate::ui::bigfont::{big, title};
use crate::ui::draw::{Cv, R, bold, fg, graph, meter, width, wrap, wrap_parts};
use crate::ui::theme::*;
use crate::util::{br, center, fmt_bytes, fmt_uptime, pyround};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};

pub fn draw(f: &mut Frame, app: &mut App) {
    let a = f.area();
    let full = R::new(a.x as i32, a.y as i32, a.width as i32, a.height as i32);
    let mut cv = Cv { buf: f.buffer_mut(), clip: full };
    cv.fill(full, BG);
    if full.w < 4 || full.h < 4 {
        return;
    }
    tabs_bar(&mut cv, app, full);
    footer(&mut cv, R::new(full.x, full.bottom() - 1, full.w, 1));
    // área das abas: tira 2 linhas de cima, 1 de baixo e 1 coluna de cada lado
    let pane = R::new(full.x + 1, full.y + 2, full.w - 2, full.h - 3);
    {
        let mut pcv = cv.clipped(pane);
        match app.tab {
            Tab::Cpu => tab_cpu(&mut pcv, app, pane),
            Tab::Gpu => tab_gpu(&mut pcv, app, pane),
            Tab::Proc => tab_proc(&mut pcv, app, pane),
        }
    }
    toasts(&mut cv, app, R::new(full.x, full.y, full.w, full.h - 1));
    if app.dialog.is_some() {
        dialog(&mut cv, app, full);
    }
}

// ───────────────────────── barra de abas e rodapé ─────────────────────────
fn tabs_bar(cv: &mut Cv, app: &mut App, full: R) {
    cv.fill(R::new(full.x, full.y, full.w, 2), BAR_BG);
    for x in full.x..full.right() {
        cv.ch(x, full.y + 1, '━', fg(HEAD_BG));
    }
    app.tab_hits.clear();
    let mut x = full.x + 1;
    for (label, tab) in [("CPU e RAM", Tab::Cpu), ("GPU", Tab::Gpu), ("Processos", Tab::Proc)] {
        let w = width(label) + 2;
        let on = app.tab == tab;
        cv.put(x + 1, full.y, label, if on { bold(TAB_ON) } else { fg(TAB_OFF) });
        if on {
            for k in 0..w {
                cv.ch(x + k, full.y + 1, '━', fg(ACCENT));
            }
        }
        app.tab_hits.push((R::new(x, full.y, w, 2), tab));
        x += w + 1;
    }
}

fn footer(cv: &mut Cv, r: R) {
    cv.fill(r, BAR_BG);
    let mut x = r.x + 1;
    for (key, desc) in [
        ("1", "CPU/RAM"),
        ("2", "GPU"),
        ("3", "Processos"),
        ("/", "Buscar"),
        ("k", "Encerrar"),
        ("+", "+Prioridade"),
        ("-", "-Prioridade"),
        ("t", "Kernel"),
        ("q", "Sair"),
    ] {
        x = cv.put(x, r.y, key, bold(FOOT_KEY));
        x = cv.put(x + 1, r.y, desc, fg(TEXT)) + 2;
    }
}

// ───────────────────────────── aba 1: CPU e RAM ─────────────────────────────
/// Cor da temperatura (m°C): verde, laranja a 15 °C do limite, vermelho no limite.
fn heat(t: i64, high: Option<i64>) -> Color {
    let high = high.unwrap_or(85_000);
    if t >= high {
        RED
    } else if t >= high - 15_000 {
        ORANGE
    } else {
        GREEN
    }
}

type DetLine = (&'static str, String, Style);

fn details(cv: &mut Cv, x: i32, y: i32, lines: &[DetLine]) -> i32 {
    let mut w = 0;
    for (i, (label, value, st)) in lines.iter().enumerate() {
        let nx = cv.put(x + 2, y + i as i32, label, fg(SUB2));
        let nx = cv.put(nx, y + i as i32, value, *st);
        w = w.max(nx - x);
    }
    x + w + 2
}

/// Verde abaixo de 70% dos núcleos ocupados, laranja até 100%, vermelho com fila.
fn load_style(v: f64, ncpu: usize) -> Style {
    let n = ncpu.max(1) as f64;
    fg(if v >= n {
        RED
    } else if v >= 0.7 * n {
        ORANGE
    } else {
        GREEN
    })
}

/// Detalhes de carga média, threads e tempo ligado (vazio se o /proc não deu nada).
fn load_lines(app: &App) -> Vec<DetLine> {
    let l = &app.load;
    let plain = fg(DET);
    let mut v: Vec<DetLine> = Vec::new();
    if let Some(a) = l.avg {
        let ncpu = app.cpu.per.len();
        v.push(("Carga média", String::new(), plain));
        for (label, x) in ["  1 min  ", "  5 min  ", " 15 min  "].into_iter().zip(a) {
            v.push((label, br(x, 2), load_style(x, ncpu)));
        }
        if l.threads > 0 {
            v.push(("Threads  ", format!("{} de {}", l.running, l.threads), plain));
        }
    }
    if let Some(u) = l.uptime {
        v.push(("Ligado   ", fmt_uptime(u), plain));
    }
    v
}

fn tab_cpu(cv: &mut Cv, app: &App, r: R) {
    let mb = |x: u64| format!("{} MB", x >> 20);
    let temps = app.cpu_temp.ok();
    let rem = r.h - 10;
    let h1 = ((rem + 1) / 2).max(4);
    // com temperatura por núcleo, o medidor cede uma linha para ela
    let per_core = temps && app.cpu.ids.iter().any(|id| app.cpu_temp.of_cpu(*id).is_some());
    let mh = if per_core { (h1 - 1).max(4) } else { h1 };
    let h2 = (rem - h1).max(4);

    let mut y = r.y + 1;
    for line in title("CPU") {
        cv.put(r.x, y, &line, bold(TITLE));
        y += 1;
    }
    let n = app.cpu.per.len().max(1) as i32;
    let mw = (r.w / n - 1).clamp(6, 11);
    // carga média: ao lado dos medidores se couber, senão na linha do nome
    let side = load_lines(app);
    let side_x = r.x + n * (mw + 1);
    let side_w = side.iter().map(|(l, v, _)| width(l) + width(v)).max().unwrap_or(0);
    let beside = side_x + 2 + side_w <= r.right();

    let mut x = cv.put(r.x, y, &app.cpu_name, fg(SUB));
    if let Some(t) = app.cpu_temp.pkg {
        x = cv.put(x, y, "  ·  ", fg(SUB2));
        x = cv.put(x, y, &format!("{} °C", pyround(t as f64 / 1000.0)), bold(heat(t, app.cpu_temp.high)));
    }
    if !beside && let Some(a) = app.load.avg {
        x = cv.put(x, y, "  ·  carga", fg(SUB2));
        for v in a {
            x = cv.put(x + 1, y, &br(v, 2), load_style(v, app.cpu.per.len()));
        }
    }
    y += 2;
    if beside {
        details(cv, side_x, y, &side[..side.len().min(h1 as usize)]);
    }

    for (i, v) in app.cpu.per.iter().enumerate() {
        let mr = R::new(r.x + i as i32 * (mw + 1), y, mw, mh);
        meter(cv, mr, *v, &format!("{} %", pyround(*v)), &format!("CPU {}", i + 1), METER_LIT, METER_OFF, METER_BORDER);
        if per_core {
            let t = app.cpu.ids.get(i).and_then(|id| app.cpu_temp.of_cpu(*id));
            let (txt, st) = match t {
                Some(t) => (format!("{} °C", pyround(t as f64 / 1000.0)), bold(heat(t, app.cpu_temp.high))),
                None => ("--".into(), fg(SUB2)),
            };
            cv.put(mr.x, y + mh, &center(&txt, mw as usize), st);
        }
    }
    y += h1 + 1;

    for line in title("MEMORIA") {
        cv.put(r.x, y, &line, bold(TITLE));
        y += 1;
    }
    cv.put(r.x, y, &app.ram_info, fg(SUB2));
    y += 2;

    let m = &app.mem;
    let mw = (r.w / 4).clamp(6, 11);
    meter(cv, R::new(r.x, y, mw, h2), m.percent, &format!("{} %", pyround(m.percent)), "RAM", METER_LIT, METER_OFF, RAM_BORDER);
    let plain = fg(DET);
    let x = details(
        cv,
        r.x + mw + 1,
        y,
        &[
            ("Em uso      ", mb(m.used), bold(METER_LIT)),
            ("Disponível  ", mb(m.available), plain),
            ("Cache       ", mb(m.cache), plain),
            ("Swap        ", format!("{} de {}", mb(m.swap_used), mb(m.swap_total)), plain),
        ],
    );

    if let Some(z) = app.zram.stats.as_ref().filter(|_| app.has_zram) {
        let pct = 100.0 * z.orig as f64 / z.size as f64;
        meter(cv, R::new(x, y, mw, h2), pct, &format!("{} %", pyround(pct)), "ZRAM", ZRAM_LIT, ZRAM_OFF, ZRAM_BORDER);
        let taxa = if z.compr > 0 { format!("{}x", br(z.orig as f64 / z.compr as f64, 1)) } else { "--".into() };
        details(
            cv,
            x + mw + 1,
            y,
            &[
                ("Guardado    ", format!("{} de {}", mb(z.orig), mb(z.size)), bold(ZRAM_LIT)),
                ("Ocupa na RAM ", mb(z.used), plain),
                ("Compressão  ", taxa, plain),
                ("Algoritmo   ", app.zram.alg.clone(), plain),
            ],
        );
    }
}

// ───────────────────────────── aba 2: GPU ─────────────────────────────
fn tab_gpu(cv: &mut Cv, app: &App, r: R) {
    let card = R::new(r.x + 1, r.y, r.w - 2, r.h);
    cv.fill(card, GPU_CARD_BG);
    cv.frame(card, GPU_BORDER);
    cv.put(card.x + 2, card.y, " ▣ GPU ", bold(GPU_TITLE));
    let inner = R::new(card.x + 3, card.y + 1, card.w - 6, card.h - 2);
    if inner.w < 6 || inner.h < 1 {
        return;
    }
    let mut cv = cv.clipped(inner);
    let s = &app.gpu.sens;
    let mib = |v: i64| pyround(v as f64 / (1u64 << 20) as f64);
    let vram = s.vram_used.zip(s.vram_total.filter(|t| *t > 0));

    // textos da linha de baixo (podem quebrar em mais de uma linha)
    let cw = inner.w / 3;
    let cx = [inner.x, inner.x + cw, inner.x + cw + (inner.w - cw + 1) / 2];
    let last_w = inner.right() - cx[2];
    let mut clk = Vec::new();
    for v in [s.sclk, s.mclk].into_iter().flatten().filter(|v| *v != 0) {
        clk.push(format!("{} MHz", v / 1_000_000));
    }
    let clk = if clk.is_empty() { "--".to_string() } else { clk.join(" / ") };
    let fan = s.fan.map(|f| format!("{f} RPM")).unwrap_or_else(|| "--".into());
    let mut pw = s.power.map(|p| format!("{} W", br(p as f64 / 1e6, 1))).unwrap_or_else(|| "--".into());
    if let Some(c) = s.cap.filter(|c| *c != 0) {
        pw.push_str(&format!(" (limite {} W)", pyround(c as f64 / 1e6)));
    }
    let fan_pw = format!("{fan}  /  {pw}");
    let vram_txt = vram.map(|(u, t)| format!("VRAM {} de {} MB", mib(u), mib(t)));
    let fan_parts: Vec<&str> = std::iter::once(fan_pw.as_str()).chain(vram_txt.as_deref()).collect();
    let info = [
        ("Nome", wrap(&app.gpu.name, cw)),
        ("Clock GPU / Memória", wrap(&clk, cx[2] - cx[1])),
        ("Ventoinha / Consumo", wrap_parts(&fan_parts, "   ·   ", last_w)),
    ];
    let info_h = 1 + info.iter().map(|(_, l)| l.len() as i32).max().unwrap_or(1);

    let rem = inner.h - (1 + 1 + 4 + 1 + info_h + 1);
    let gh = ((rem * 2 + 1) / 3).max(3);
    let vh = (rem - gh).max(2);

    let mut y = inner.y + 1;
    graph(&mut cv, R::new(inner.x, y, inner.w, gh), &app.gpu_hist, PAL_MODERN, GPU_GRAPH_BG);
    y += gh + 1;

    // números grandes
    let util = app.gpu_use.map(|g| format!("{}%", pyround(g))).unwrap_or_else(|| "--".into());
    let vram_big = vram.map(|(u, _)| mib(u).to_string()).unwrap_or_else(|| "--".into());
    let temp = s.temp.map(|t| format!("{}°C", pyround(t as f64 / 1000.0))).unwrap_or_else(|| "--".into());
    for (i, (label, value)) in [("Utilização", util), ("VRAM em uso (MB)", vram_big), ("Temperatura", temp)].iter().enumerate() {
        cv.put(cx[i], y, label, fg(GPU_LBL));
        for (k, line) in big(value).iter().enumerate() {
            cv.put(cx[i], y + 1 + k as i32, line, bold(GPU_BIG));
        }
    }
    y += 5;

    for (i, (label, lines)) in info.iter().enumerate() {
        cv.put(cx[i], y, label, fg(GPU_LBL));
        for (k, line) in lines.iter().enumerate() {
            cv.put(cx[i], y + 1 + k as i32, line, fg(GPU_VAL));
        }
    }
    y += info_h + 1;

    graph(&mut cv, R::new(inner.x, y, inner.w, vh), &app.vram_hist, PAL_MODERN2, VRAM_GRAPH_BG);
}

// ───────────────────────────── aba 3: Processos ─────────────────────────────
fn section(cv: &mut Cv, r: R, y: i32, text: &str) {
    cv.fill(R::new(r.x, y, r.w, 1), HEAD_BG);
    cv.put(r.x + 1, y, text, bold(TITLE).add_modifier(Modifier::ITALIC));
}

fn dot(cv: &mut Cv, x: i32, y: i32, color: Color, label: &str) -> i32 {
    cv.put(x, y, "●", bold(color));
    cv.put(x + 2, y, label, fg(SOFT))
}

/// Larguras das colunas da tabela de processos; encolhe o que dá para caber.
fn proc_cols(avail: i32) -> [i32; 8] {
    let mut w = [18, 28, 8, 10, 8, 11, 17, 8];
    let min = [15, 12, 7, 8, 7, 9, 16, 7];
    let mut over = w.iter().map(|c| c + 2).sum::<i32>() - avail;
    for i in [0, 6, 2, 7, 4, 5, 3, 1] {
        if over <= 0 {
            break;
        }
        let cut = (w[i] - min[i]).min(over);
        w[i] -= cut;
        over -= cut;
    }
    w
}

fn tab_proc(cv: &mut Cv, app: &mut App, r: R) {
    let mut y = r.y;
    section(cv, r, y, "Visão geral dos recursos");
    y += 2;

    // ── visão geral ──
    let ov = R::new(r.x, y, r.w, 8);
    cv.fill(ov, OV_BG);
    cv.fill(R::new(r.x, y, r.w, 1), HEAD_BG);
    let ocols = [12, 14, 12];
    let ox = [r.x + 1, r.x + 1 + ocols[0] + 2, r.x + 1 + ocols[0] + ocols[1] + 4, r.x + 1 + ocols[0] + ocols[1] + ocols[2] + 6];
    for (i, h) in ["Componente", "Status", "Utilização", "Detalhes"].iter().enumerate() {
        cv.put(ox[i], y, h, bold(TITLE));
    }
    {
        let mut ocv = cv.clipped(ov);
        for (i, row) in app.overview.iter().enumerate() {
            let ry = y + 1 + i as i32;
            ocv.put_w(ox[0], ry, ocols[0], row.comp, bold(TITLE), false);
            dot(&mut ocv, ox[1], ry, row.dot, row.status);
            ocv.put_w(ox[2], ry, ocols[2], &row.util, fg(TEXT), false);
            ocv.put(ox[3], ry, &row.det, fg(TEXT));
        }
    }
    y += 9;

    section(cv, r, y, "Processos");
    y += 1;
    let x = dot(cv, r.x, y, GREEN, "ativo   ");
    let x = dot(cv, x, y, GRAY, "segundo plano   ");
    dot(cv, x, y, RED, "pesado (CPU ≥ 50% de um núcleo ou RAM ≥ 10%)");
    y += 1;

    // ── campo de busca ──
    if app.filter_open {
        let fr = R::new(r.x, y, r.w, 3);
        // fundo igual ao da página: um fundo próprio sobraria em volta da borda
        cv.fill(fr, BG);
        cv.frame(fr, if app.filter_focus { ACCENT } else { METER_BORDER });
        let mut icv = cv.clipped(R::new(fr.x + 2, y + 1, fr.w - 4, 1));
        let mut cx = fr.x + 2;
        if app.filter.is_empty() {
            icv.put(cx, y + 1, "Buscar processo pelo nome… (Esc fecha)", fg(PLACEHOLDER));
        } else {
            cx = icv.put(cx, y + 1, &app.filter, fg(TITLE));
        }
        if app.filter_focus {
            let under = if app.filter.is_empty() { 'B' } else { ' ' };
            icv.ch(cx, y + 1, under, Style::new().fg(INPUT_BG).bg(TITLE));
        }
        y += 4;
    }

    // ── tabela ──
    let table = R::new(r.x, y, r.w, r.bottom() - y);
    cv.fill(table, TABLE_BG);
    if table.h < 1 {
        app.table_rows = R::default();
        return;
    }
    cv.fill(R::new(table.x, y, table.w, 1), HEAD_BG);
    let w = proc_cols(table.w - 1);
    let mut xs = [0i32; 8];
    let mut x = table.x + 1;
    for i in 0..8 {
        xs[i] = x;
        x += w[i] + 2;
    }
    let mut tcv = cv.clipped(table);
    for (i, h) in ["Status", "Processo", "PID", "Usuário", "CPU", "Memória", "Prioridade", "Threads"].iter().enumerate() {
        tcv.put_w(xs[i], y, w[i], h, bold(TITLE), false);
    }
    let rows = R::new(table.x, y + 1, table.w, table.h - 1);
    app.table_rows = rows;
    let vis = rows.h.max(0) as usize;
    let total = app.view.len();
    app.scroll = app.scroll.min(total.saturating_sub(vis.max(1)));
    let plain = fg(TEXT);
    for k in 0..vis {
        let idx = app.scroll + k;
        let Some(&pi) = app.view.get(idx) else { break };
        let p = &app.procs.list[pi];
        let ry = rows.y + k as i32;
        let selected = idx == app.cursor;
        if selected {
            tcv.fill(R::new(rows.x, ry, rows.w, 1), CURSOR_BG);
        } else if idx % 2 == 1 {
            tcv.fill(R::new(rows.x, ry, rows.w, 1), TABLE_ALT);
        }
        let (color, label) = match p.rank {
            Rank::Pesado => (RED, "Pesado"),
            Rank::Ativo => (GREEN, "Ativo"),
            Rank::SegundoPlano => (GRAY, "Segundo plano"),
        };
        {
            let mut c = tcv.clipped(R::new(xs[0], ry, w[0], 1));
            dot(&mut c, xs[0], ry, color, label);
        }
        let mine = p.uid == Some(app.me);
        let name_st = if p.rank == Rank::Pesado {
            bold(WHITE)
        } else if mine {
            fg(SOFT)
        } else {
            fg(OTHER_USER)
        };
        tcv.put_w(xs[1], ry, w[1], &p.name, name_st, false);
        tcv.put_w(xs[2], ry, w[2], &p.pid.to_string(), plain, false);
        let user = p.uid.and_then(|u| app.procs.users.get(&u)).map_or("?", String::as_str);
        tcv.put_w(xs[3], ry, w[3].min(10), user, plain, false);
        tcv.put_w(xs[4], ry, w[4], &format!("{} %", br(p.cpu, 1)), fg(if p.cpu >= 50.0 { RED } else { SOFT }), true);
        tcv.put_w(xs[5], ry, w[5], &fmt_bytes(p.rss as f64), plain, true);
        let pcolor = if p.nice < 0 {
            ORANGE
        } else if p.nice > 0 {
            GRAY
        } else {
            SOFT
        };
        tcv.put_w(xs[6], ry, w[6], nivel_nome(p.nice), fg(pcolor), false);
        tcv.put_w(xs[7], ry, w[7], &p.threads.to_string(), plain, true);
    }
    // barra de rolagem
    if total > vis && vis > 0 {
        let thumb = (vis * vis / total).max(1);
        let pos = app.scroll * (vis - thumb) / (total - vis);
        for k in 0..vis {
            let on = k >= pos && k < pos + thumb;
            tcv.ch(rows.right() - 1, rows.y + k as i32, ' ', Style::new().bg(if on { METER_BORDER } else { OV_BG }));
        }
    }
}

// ───────────────────────────── diálogo e avisos ─────────────────────────────
fn dialog(cv: &mut Cv, app: &mut App, full: R) {
    let Some(d) = &app.dialog else { return };
    let w = 60.min(full.w - 2);
    let h = 9;
    let r = R::new(full.x + (full.w - w) / 2, full.y + (full.h - h) / 2, w, h);
    cv.fill(r, DIALOG_BG);
    cv.frame(r, RED);
    let mut c = cv.clipped(r.shrink(1, 1));
    let x = c.put(r.x + 3, r.y + 2, "Encerrar ", fg(TEXT));
    let x = c.put(x, r.y + 2, &d.name, bold(WHITE));
    c.put(x, r.y + 2, &format!(" (PID {})?", d.pid), fg(TEXT));

    let (l_yes, l_no) = ("Sim, encerrar", "Não");
    let (w_yes, w_no) = (width(l_yes) + 4, width(l_no) + 4);
    let x_no = r.right() - 3 - w_no;
    let x_yes = x_no - 2 - w_yes;
    let by = r.y + 4;
    let mut button = |x: i32, w: i32, label: &str, on: bool, color: Color, dim: Color| -> R {
        let br = R::new(x, by, w, 3);
        c.fill(br, if on { color } else { dim });
        let st = if on { bold(WHITE).add_modifier(Modifier::UNDERLINED) } else { fg(OTHER_USER) };
        c.put(x + 2, by + 1, label, st);
        br
    };
    let hit_yes = button(x_yes, w_yes, l_yes, d.yes, BTN_ERR, BTN_ERR_OFF);
    let hit_no = button(x_no, w_no, l_no, !d.yes, BTN_PRI, BTN_PRI_OFF);
    app.dialog_hits = [hit_yes, hit_no];
}

fn toasts(cv: &mut Cv, app: &App, area: R) {
    let mut bottom = area.bottom() - 1;
    for t in app.toasts.iter().rev() {
        let w = (width(&t.text) + 5).min(area.w - 2);
        let r = R::new(area.right() - w - 1, bottom - 3, w, 3);
        if r.y < area.y || w < 6 {
            break;
        }
        cv.fill(r, HEAD_BG);
        let color = match t.sev {
            Sev::Info => ACCENT,
            Sev::Warn => ORANGE,
            Sev::Error => RED,
        };
        for k in 0..3 {
            cv.ch(r.x, r.y + k, '▌', fg(color));
        }
        cv.put_w(r.x + 2, r.y + 1, w - 3, &t.text, fg(TITLE), false);
        bottom -= 4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_cols_encolhe_ate_caber() {
        let soma = |w: [i32; 8]| w.iter().map(|c| c + 2).sum::<i32>();
        assert_eq!(proc_cols(200), [18, 28, 8, 10, 8, 11, 17, 8]);
        for avail in [100, 120, 140] {
            assert!(soma(proc_cols(avail)) <= avail, "não coube em {avail}");
        }
        // abaixo do mínimo de todas as colunas não encolhe mais (o recorte cuida do resto)
        assert_eq!(proc_cols(10), [15, 12, 7, 8, 7, 9, 16, 7]);
    }
}
