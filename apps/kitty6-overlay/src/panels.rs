//! The HUD's panels: a strip down the left edge (frames, CPU and GPU,
//! draws, pipelines, the bottleneck) and one down the right (memory, the
//! card, disk, threads, stalls, the layer's holdings). The centre stays
//! clear for the game.

use victory_core::telemetry::{FrameRing, Second, Stats};
use victory_core::verdict::{Cause, Kind, PAGING};

use crate::model::{graph_top, nice_top, severity, short, size};
use crate::paint::*;

/// Everything a draw needs.
pub struct View<'a> {
    /// The game runs and announced itself.
    pub connected: bool,
    /// Connected, but no second has arrived lately (the sampling starting).
    pub syncing: bool,
    pub pid: u32,
    pub frames: &'a FrameRing,
    pub stats: &'a Stats,
    pub causes: &'a [Cause],
    /// The hotkey works (else F6 is polled).
    pub hotkey: bool,
    pub demo: bool,
    /// Flips twice a second, for blinking marks.
    pub blink: bool,
}

/// Pixels of a strip at scale 1.
pub const STRIP_WIDTH: f32 = 400.0;
/// Height of the header alone (shown while waiting for the game).
pub const HEADER_HEIGHT: f32 = 78.0;

fn seconds(stats: &Stats, n: usize) -> Vec<&Second> {
    let all: Vec<&Second> = stats.history().collect();
    all[all.len().saturating_sub(n)..].to_vec()
}

fn kind_colour(k: Kind) -> Rgba {
    match k {
        Kind::Ram => RED,
        Kind::CardMemory => MAGENTA,
        Kind::Pipelines => AMBER,
        Kind::Preparing => CYAN,
        Kind::Gpu => VIOLET,
        Kind::Nothing => GREEN,
    }
}

/// The left strip.
pub fn left(p: &Painter, w: f32, v: &View) {
    let s = p.scale;
    let m = 10.0 * s;
    let pw = w - 2.0 * m;
    let gap = 8.0 * s;
    let mut y = m;
    header(p, Rect::new(m, y, pw, 58.0 * s), v);
    y += 58.0 * s + gap;
    if !v.connected {
        return;
    }
    for (height, panel) in [
        (236.0, frame_panel as fn(&Painter, Rect, &View)),
        (128.0, cpu_gpu_panel),
        (100.0, draws_panel),
        (104.0, pipelines_panel),
        (122.0, bottleneck_panel),
    ] {
        let r = Rect::new(m, y, pw, height * s);
        panel(p, r, v);
        y += r.h + gap;
    }
}

/// The right strip.
pub fn right(p: &Painter, w: f32, v: &View) {
    if !v.connected {
        return;
    }
    let s = p.scale;
    let m = 10.0 * s;
    let pw = w - 2.0 * m;
    let gap = 8.0 * s;
    let mut y = m;
    for (height, panel) in [
        (214.0, memory_panel as fn(&Painter, Rect, &View)),
        (108.0, vram_panel),
        (80.0, disk_panel),
        (162.0, threads_panel),
        (118.0, stalls_panel),
        (176.0, layer_panel),
    ] {
        let r = Rect::new(m, y, pw, height * s);
        panel(p, r, v);
        y += r.h + gap;
    }
}

fn header(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    p.fill(r, PANEL);
    let mut y = r.y + 2.0 * s;
    while y < r.bottom() {
        p.line(r.x, y, r.right(), y, CYAN.alpha(0.03), 1.0);
        y += 3.0 * s;
    }
    // A slanted accent bar and brackets.
    p.fill(Rect::new(r.x, r.y, 4.0 * s, r.h), CYAN);
    p.line(
        r.x,
        r.bottom(),
        r.right(),
        r.bottom(),
        CYAN.alpha(0.6),
        1.2 * s,
    );
    p.line(r.right() - 14.0 * s, r.y, r.right(), r.y, CYAN, 1.8 * s);
    p.line(r.right(), r.y, r.right(), r.y + 14.0 * s, CYAN, 1.8 * s);

    let title = Rect::new(r.x + 14.0 * s, r.y + 5.0 * s, r.w * 0.6, 26.0 * s);
    p.glow_text(title, "KITTY6", LARGE, CYAN, Align::Left);
    p.text(
        Rect::new(r.x + 104.0 * s, r.y + 13.0 * s, r.w * 0.6, 14.0 * s),
        if v.demo {
            "// TELEMETRY  [DEMO]"
        } else {
            "// TELEMETRY"
        },
        TINY,
        TEXT.alpha(0.8),
        Align::Left,
    );

    // The key hint.
    let key = Rect::new(r.right() - 58.0 * s, r.y + 9.0 * s, 26.0 * s, 20.0 * s);
    p.fill(key, CYAN.alpha(0.12));
    p.stroke(key, CYAN.alpha(0.8), 1.0);
    p.text(key.inset(0.0, 3.0 * s), "F6", SMALL, CYAN, Align::Center);
    p.text(
        Rect::new(key.right() + 3.0 * s, key.y + 4.0 * s, 30.0 * s, 12.0 * s),
        if v.hotkey { "HIDE" } else { "HIDE*" },
        TINY,
        DIM,
        Align::Left,
    );

    let status = Rect::new(r.x + 26.0 * s, r.y + 37.0 * s, r.w - 34.0 * s, 14.0 * s);
    let st = v.stats;
    let (dot, text) = if !v.connected {
        (
            MAGENTA,
            "WAITING FOR KITTY6\u{2026}  (start the game)".to_owned(),
        )
    } else if v.syncing {
        (
            AMBER,
            format!("LINK UP \u{00b7} PID {} \u{00b7} SYNCING\u{2026}", v.pid),
        )
    } else {
        let mut t = format!("ONLINE \u{00b7} PID {}", v.pid);
        if st.width > 0 {
            t += &format!(" \u{00b7} {}\u{00d7}{}", st.width, st.height);
        }
        t += &format!(
            " \u{00b7} FRAME {} \u{00b7} T+{:.0}s",
            st.title_frame, st.now.t_s
        );
        (GREEN, t)
    };
    if v.blink || v.connected && !v.syncing {
        p.dot(r.x + 17.0 * s, status.y + 6.5 * s, 3.2 * s, dot);
    }
    p.text(
        status,
        &text,
        SMALL,
        if v.connected { TEXT } else { MAGENTA },
        Align::Left,
    );
}

fn frame_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "01", "FRAME TIME // LAST 512 PRESENTS", CYAN);
    let now = &v.stats.now;
    let sev = severity(if now.ms_p50 > 0.0 {
        now.ms_p50
    } else {
        now.ms_mean
    });
    let c = severity_colour(sev);
    let fps_text = if now.fps >= 10.0 {
        format!("{:.0}", now.fps)
    } else {
        format!("{:.1}", now.fps)
    };
    p.glow_text(
        Rect::new(inner.x, inner.y - 6.0 * s, 120.0 * s, 52.0 * s),
        &fps_text,
        HUGE,
        c,
        Align::Left,
    );
    p.text(
        Rect::new(inner.x + 2.0 * s, inner.y + 42.0 * s, 60.0 * s, 12.0 * s),
        "FPS",
        TINY,
        c.alpha(0.8),
        Align::Left,
    );

    // Spread of the last second.
    let cols = [
        ("P50", now.ms_p50),
        ("P95", now.ms_p95),
        ("MAX", now.ms_max),
    ];
    let cx = inner.x + 118.0 * s;
    let cw = (inner.right() - cx) / 3.0;
    for (i, (label, ms)) in cols.iter().enumerate() {
        let x = cx + cw * i as f32;
        p.text(
            Rect::new(x, inner.y, cw, 12.0 * s),
            label,
            TINY,
            DIM,
            Align::Left,
        );
        p.text(
            Rect::new(x, inner.y + 11.0 * s, cw, 18.0 * s),
            &format!("{ms:.1}"),
            MEDIUM,
            severity_colour(severity(*ms)),
            Align::Left,
        );
    }
    p.text(
        Rect::new(cx, inner.y + 32.0 * s, inner.right() - cx, 12.0 * s),
        &format!(
            "MS \u{00b7} LATE {} \u{00b7} SLOW {} \u{00b7} >500 {}",
            now.over33, now.over100, now.over500
        ),
        TINY,
        DIM,
        Align::Left,
    );

    let g = Rect::new(
        inner.x,
        inner.y + 58.0 * s,
        inner.w - 22.0 * s,
        inner.h - 60.0 * s,
    );
    let kept: Vec<(f32, u32)> = v.frames.iter().collect();
    // Scaled to the 95th percentile (the 100 ms guide always in view);
    // taller spikes are cut at the top and marked.
    let mut sorted: Vec<f32> = kept.iter().map(|f| f.0).collect();
    sorted.sort_by(f32::total_cmp);
    let p95 = sorted
        .get((sorted.len() * 95 / 100).min(sorted.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    let top = graph_top((p95 * 1.3).max(110.0));
    p.grid(g, 4, 8);
    let y_of = |ms: f32| g.bottom() - g.h * (ms / top).clamp(0.0, 1.0);
    for (ms, c) in [(33.3, GREEN), (50.0, AMBER), (100.0, MAGENTA)] {
        if ms < top {
            let y = y_of(ms);
            p.dashed(g.x, y, g.right(), y, c.alpha(0.55), 1.0);
            p.text(
                Rect::new(g.right() + 2.0 * s, y - 6.0 * s, 22.0 * s, 12.0 * s),
                &format!("{ms:.0}"),
                TINY,
                c,
                Align::Left,
            );
        }
    }
    p.text(
        Rect::new(g.right() + 2.0 * s, g.y - 2.0 * s, 22.0 * s, 12.0 * s),
        &short(top),
        TINY,
        DIM,
        Align::Left,
    );
    let slot = g.w / victory_core::telemetry::FRAMES as f32;
    let start = victory_core::telemetry::FRAMES - kept.len();
    let pts: Vec<(f32, f32, u8)> = kept
        .iter()
        .enumerate()
        .map(|(i, &(ms, _))| {
            (
                g.x + slot * (start + i) as f32 + slot * 0.5,
                y_of(ms),
                severity(ms),
            )
        })
        .collect();
    p.clip(g, || {
        for &(x, y, sv) in &pts {
            if sv > 0 {
                p.line(
                    x,
                    y,
                    x,
                    g.bottom(),
                    severity_colour(sv).alpha(0.22),
                    slot.max(1.0),
                );
            }
        }
        // The line, one run per severity.
        let mut run: Vec<(f32, f32)> = Vec::new();
        let mut run_sev = pts.first().map_or(0, |pt| pt.2);
        for &(x, y, sv) in &pts {
            if sv != run_sev && !run.is_empty() {
                let last = *run.last().unwrap_or(&(x, y));
                p.polyline(&run, severity_colour(run_sev), 1.2 * s);
                run.clear();
                run.push(last);
                run_sev = sv;
            }
            run.push((x, y));
        }
        if run.len() > 1 {
            p.polyline(&run, severity_colour(run_sev), 1.2 * s);
        }
        for (&(x, _, _), &(ms, _)) in pts.iter().zip(&kept) {
            if ms > top {
                p.dot(x, g.y + 2.5 * s, 2.2 * s, MAGENTA);
            }
        }
    });
    if let Some(&(ms, faults)) = kept.last() {
        p.fill(
            Rect::new(g.right() - 128.0 * s, g.y + 1.0 * s, 128.0 * s, 13.0 * s),
            PANEL.alpha(0.9),
        );
        p.text(
            Rect::new(g.x + 4.0 * s, g.y + 2.0 * s, g.w - 8.0 * s, 12.0 * s),
            &format!("LAST {ms:.1} ms \u{00b7} {} PF", short(faults as f32)),
            TINY,
            TEXT.alpha(0.85),
            Align::Right,
        );
    }
}

fn cpu_gpu_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "02", "CPU PREP vs GPU // ms PER FRAME, 60 s", MAGENTA);
    let now = &v.stats.now;
    let legend = [
        ("PREP", now.prep_ms, CYAN),
        ("GPU", now.gpu_ms, MAGENTA),
        ("FRAME", now.ms_mean, AMBER),
    ];
    let lw = inner.w / 3.0;
    for (i, (label, ms, c)) in legend.iter().enumerate() {
        let x = inner.x + lw * i as f32;
        p.fill(Rect::new(x, inner.y + 4.0 * s, 6.0 * s, 6.0 * s), *c);
        p.text(
            Rect::new(x + 10.0 * s, inner.y, lw, 13.0 * s),
            &format!("{label} {ms:.1}"),
            BODY_BOLD,
            *c,
            Align::Left,
        );
    }
    let g = Rect::new(inner.x, inner.y + 18.0 * s, inner.w, inner.h - 18.0 * s);
    p.grid(g, 3, 6);
    let hist = seconds(v.stats, 60);
    let top = nice_top(
        hist.iter()
            .map(|h| (h.prep_ms + h.gpu_ms).max(h.ms_mean))
            .fold(10.0, f32::max),
    );
    let bw = g.w / 60.0;
    let start = 60 - hist.len();
    let mut frame_line = Vec::with_capacity(hist.len());
    for (i, h) in hist.iter().enumerate() {
        let x = g.x + bw * (start + i) as f32;
        let ph = g.h * (h.prep_ms / top).clamp(0.0, 1.0);
        let gh = g.h * (h.gpu_ms / top).clamp(0.0, 1.0 - ph / g.h.max(1.0));
        p.fill(
            Rect::new(x + 0.5, g.bottom() - ph, bw - 1.0, ph),
            CYAN.alpha(0.65),
        );
        p.fill(
            Rect::new(x + 0.5, g.bottom() - ph - gh, bw - 1.0, gh),
            MAGENTA.alpha(0.6),
        );
        frame_line.push((
            x + bw * 0.5,
            g.bottom() - g.h * (h.ms_mean / top).clamp(0.0, 1.0),
        ));
    }
    p.polyline(&frame_line, AMBER, 1.2 * s);
    p.text(
        Rect::new(g.x + 2.0 * s, g.y + 1.0 * s, 60.0 * s, 12.0 * s),
        &format!("{} ms", short(top)),
        TINY,
        DIM,
        Align::Left,
    );
}

fn draws_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "03", "DRAWS / DROPPED // PER FRAME", GREEN);
    let now = &v.stats.now;
    p.text(
        Rect::new(inner.x, inner.y, inner.w / 2.0, 14.0 * s),
        &format!("DRAWS {}", short(now.draws)),
        BODY_BOLD,
        GREEN,
        Align::Left,
    );
    let dropped_c = if now.dropped > 50.0 { MAGENTA } else { DIM };
    p.text(
        Rect::new(inner.x + inner.w / 2.0, inner.y, inner.w / 2.0, 14.0 * s),
        &format!("DROPPED {}", short(now.dropped)),
        BODY_BOLD,
        dropped_c,
        Align::Right,
    );
    let g = Rect::new(inner.x, inner.y + 17.0 * s, inner.w, inner.h - 17.0 * s);
    p.grid(g, 2, 6);
    let hist = seconds(v.stats, 120);
    let draws: Vec<f32> = hist.iter().map(|h| h.draws).collect();
    let dropped: Vec<f32> = hist.iter().map(|h| h.dropped).collect();
    p.sparkline(
        g,
        &draws,
        nice_top(draws.iter().copied().fold(0.0, f32::max)),
        GREEN,
    );
    p.sparkline(
        g,
        &dropped,
        nice_top(dropped.iter().copied().fold(0.0, f32::max).max(50.0)),
        MAGENTA,
    );
}

fn pipelines_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "04", "HOST PIPELINES", AMBER);
    let now = &v.stats.now;
    let waiting_c = if now.pipelines_waiting > 0.0 {
        AMBER
    } else {
        GREEN
    };
    p.glow_text(
        Rect::new(inner.x, inner.y - 2.0 * s, 90.0 * s, 26.0 * s),
        &short(now.pipelines_waiting),
        LARGE,
        waiting_c,
        Align::Left,
    );
    p.text(
        Rect::new(inner.x, inner.y + 24.0 * s, 90.0 * s, 12.0 * s),
        "WAITING",
        TINY,
        DIM,
        Align::Left,
    );
    let g = Rect::new(
        inner.x + 92.0 * s,
        inner.y,
        inner.w - 92.0 * s,
        inner.h - 22.0 * s,
    );
    p.grid(g, 2, 4);
    let made: Vec<f32> = seconds(v.stats, 90)
        .iter()
        .map(|h| h.pipelines_made)
        .collect();
    p.sparkline(
        g,
        &made,
        nice_top(made.iter().copied().fold(0.0, f32::max).max(5.0)),
        AMBER,
    );
    p.text(
        Rect::new(g.x + 2.0 * s, g.y + 1.0 * s, g.w, 12.0 * s),
        "MADE/s",
        TINY,
        DIM,
        Align::Left,
    );
    p.text(
        Rect::new(inner.x, inner.bottom() - 13.0 * s, inner.w, 13.0 * s),
        &format!(
            "MADE {:.0}/s \u{00b7} {:.0} ms EACH (MAX {:.0}) \u{00b7} WAITED {:.0} ms \u{00b7} REFUSED {:.0}",
            now.pipelines_made, now.pipeline_ms_mean, now.pipeline_ms_max, now.pipeline_wait_ms, now.pipelines_refused
        ),
        SMALL,
        TEXT,
        Align::Left,
    );
}

fn bottleneck_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let Some(first) = v.causes.first() else {
        return;
    };
    let c = kind_colour(first.kind);
    let inner = p.panel(r, "05", "LIKELIEST BOTTLENECK // LAST 30 s", c);
    let mark = if first.kind == Kind::Nothing {
        "\u{25c6}"
    } else {
        "\u{25b2}"
    };
    p.glow_text(
        Rect::new(inner.x, inner.y - 2.0 * s, inner.w, 20.0 * s),
        &format!("{mark} {}", first.kind.short()),
        MEDIUM,
        c,
        Align::Left,
    );
    if let Some(next) = v.causes.get(1) {
        p.text(
            Rect::new(inner.x, inner.y + 1.0 * s, inner.w, 14.0 * s),
            &format!("THEN {}", next.kind.short()),
            TINY,
            kind_colour(next.kind).alpha(0.8),
            Align::Right,
        );
    }
    let mut text = first.text.clone();
    if text.chars().count() > 300 {
        text = text.chars().take(297).collect::<String>() + "\u{2026}";
    }
    p.text(
        Rect::new(inner.x, inner.y + 21.0 * s, inner.w, inner.h - 21.0 * s),
        &text,
        SMALL_WRAP,
        TEXT.alpha(0.85),
        Align::Left,
    );
}

fn memory_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "06", "MEMORY // PROCESS & SYSTEM", CYAN);
    let n = &v.stats.now;
    let ram = n.total_ram_mb.max(1.0);
    let used = (n.total_ram_mb - n.avail_ram_mb).max(0.0);
    let commit_used = (n.commit_limit_mb - n.avail_commit_mb).max(0.0);
    let gh = 25.0 * s;
    let rows = [
        (
            "PROCESS PRIVATE / RAM",
            format!("{} / {}", size(n.private_mb), size(ram)),
            n.private_mb / ram,
            VIOLET,
        ),
        ("WORKING SET (IN RAM)", size(n.ws_mb), n.ws_mb / ram, CYAN),
        (
            "SYSTEM RAM USED",
            format!(
                "{} \u{00b7} FREE {} \u{00b7} {:.0}%",
                size(used),
                size(n.avail_ram_mb),
                n.mem_load_pct
            ),
            used / ram,
            if n.avail_ram_mb < 1024.0 { RED } else { GREEN },
        ),
        (
            "COMMIT",
            format!("{} / {}", size(commit_used), size(n.commit_limit_mb)),
            commit_used / n.commit_limit_mb.max(1.0),
            AMBER,
        ),
    ];
    for (i, (label, value, frac, c)) in rows.iter().enumerate() {
        p.gauge(
            Rect::new(inner.x, inner.y + gh * i as f32, inner.w, gh - 4.0 * s),
            label,
            value,
            *frac,
            *c,
            None,
        );
    }
    let y = inner.y + gh * 4.0 + 2.0 * s;
    let paging = n.sys_pages_in_per_s;
    let hard = paging >= PAGING as f32;
    let pc = if hard { RED } else { GREEN };
    p.text(
        Rect::new(inner.x, y, inner.w, 12.0 * s),
        "PAGING // SYSTEM PAGES IN/s",
        TINY,
        DIM,
        Align::Left,
    );
    p.text(
        Rect::new(inner.x, y - 1.0 * s, inner.w, 13.0 * s),
        &format!(
            "{}{} \u{00b7} PF {}/s",
            if hard && v.blink { "\u{25b2} " } else { "" },
            short(paging),
            short(n.faults_per_s)
        ),
        SMALL,
        pc,
        Align::Right,
    );
    let g = Rect::new(
        inner.x,
        y + 14.0 * s,
        inner.w,
        inner.bottom() - y - 14.0 * s,
    );
    p.grid(g, 2, 6);
    let pages: Vec<f32> = seconds(v.stats, 120)
        .iter()
        .map(|h| h.sys_pages_in_per_s)
        .collect();
    let top = nice_top(
        pages
            .iter()
            .copied()
            .filter(|x| x.is_finite())
            .fold(2000.0, f32::max),
    );
    let ly = g.bottom() - g.h * (PAGING as f32 / top);
    p.dashed(g.x, ly, g.right(), ly, RED.alpha(0.6), 1.0);
    p.text(
        Rect::new(g.x + 2.0 * s, ly - 11.0 * s, 60.0 * s, 11.0 * s),
        "HARD",
        TINY,
        RED.alpha(0.8),
        Align::Left,
    );
    p.sparkline(g, &pages, top, pc);
}

fn vram_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "07", "CARD MEMORY // VRAM", MAGENTA);
    let n = &v.stats.now;
    let frac = n.vram_used_mb / n.vram_budget_mb.max(1.0);
    let c = if frac >= 0.95 {
        RED
    } else if frac >= 0.85 {
        AMBER
    } else {
        MAGENTA
    };
    let gh = 30.0 * s;
    p.gauge(
        Rect::new(inner.x, inner.y, inner.w, gh - 4.0 * s),
        "USED / BUDGET",
        &format!(
            "{} / {} \u{00b7} {:.0}%",
            size(n.vram_used_mb),
            size(n.vram_budget_mb),
            frac * 100.0
        ),
        frac,
        c,
        Some(0.95),
    );
    p.gauge(
        Rect::new(inner.x, inner.y + gh, inner.w, gh - 4.0 * s),
        "SHARED (PC RAM USED BY THE CARD)",
        &size(n.vram_shared_mb),
        n.vram_shared_mb / 2048.0,
        if n.vram_shared_mb > 1024.0 {
            RED
        } else {
            VIOLET
        },
        Some(0.5),
    );
}

fn disk_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "08", "DISK // PROCESS READS", VIOLET);
    let reads: Vec<f32> = seconds(v.stats, 120)
        .iter()
        .map(|h| h.io_read_mb_s)
        .collect();
    p.text(
        Rect::new(inner.x, inner.y - 2.0 * s, inner.w, 14.0 * s),
        &format!("{:.1} MB/s", v.stats.now.io_read_mb_s),
        BODY_BOLD,
        VIOLET,
        Align::Right,
    );
    let g = Rect::new(inner.x, inner.y + 12.0 * s, inner.w, inner.h - 12.0 * s);
    p.grid(g, 2, 6);
    p.sparkline(
        g,
        &reads,
        nice_top(reads.iter().copied().fold(1.0, f32::max)),
        VIOLET,
    );
}

fn threads_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "09", "CPU // BUSIEST THREADS (CORES)", GREEN);
    let st = v.stats;
    let logical = st.cpu_logical.max(1) as f32;
    p.gauge(
        Rect::new(inner.x, inner.y, inner.w, 22.0 * s),
        "PROCESS",
        &format!("{:.1} / {:.0} CORES", st.now.cpu_cores, logical),
        st.now.cpu_cores / logical,
        GREEN,
        None,
    );
    let row = 15.0 * s;
    let mut y = inner.y + 27.0 * s;
    for t in st.threads.iter().filter(|t| !t.name().is_empty()).take(5) {
        let c = if t.cores > 0.9 {
            RED
        } else if t.cores > 0.6 {
            AMBER
        } else {
            CYAN
        };
        p.text(
            Rect::new(inner.x, y, inner.w * 0.5, row),
            t.name(),
            SMALL,
            TEXT,
            Align::Left,
        );
        let bar = Rect::new(
            inner.x + inner.w * 0.52,
            y + 4.0 * s,
            inner.w * 0.32,
            6.0 * s,
        );
        p.fill(bar, CYAN.alpha(0.07));
        p.fill(
            Rect::new(bar.x, bar.y, bar.w * t.cores.clamp(0.0, 1.0), bar.h),
            c.alpha(0.8),
        );
        p.text(
            Rect::new(inner.x + inner.w * 0.84, y, inner.w * 0.16, row),
            &format!("{:.2}", t.cores),
            SMALL,
            c,
            Align::Right,
        );
        y += row;
    }
    let kinds: Vec<String> = st
        .groups
        .iter()
        .filter(|g| !g.name().is_empty())
        .take(4)
        .map(|g| format!("{} {:.1}", g.name(), g.cores))
        .collect();
    if !kinds.is_empty() {
        p.text(
            Rect::new(inner.x, inner.bottom() - 12.0 * s, inner.w, 12.0 * s),
            &format!("KINDS: {}", kinds.join(" \u{00b7} ")),
            TINY,
            DIM,
            Align::Left,
        );
    }
}

fn stalls_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let st = v.stats;
    let inner = p.panel(
        r,
        "10",
        &format!("STALL LOG // FRAMES \u{2265} {:.0} ms", st.stall_ms),
        RED,
    );
    let row = 15.0 * s;
    let cols = [0.0, 0.2, 0.42, 0.62, 0.8];
    for (i, h) in ["T+", "FRAME", "FAULTS", "PIPES", "PG IN/s"]
        .iter()
        .enumerate()
    {
        p.text(
            Rect::new(
                inner.x + inner.w * cols[i],
                inner.y,
                inner.w * 0.2,
                12.0 * s,
            ),
            h,
            TINY,
            DIM,
            Align::Left,
        );
    }
    let mut y = inner.y + 13.0 * s;
    let mut any = false;
    for stall in st.stalls().take(5) {
        any = true;
        let cells = [
            format!("{:.0}s", stall.t_s),
            format!("{:.0} ms", stall.ms),
            short(stall.faults),
            format!("{:.0}/{:.0}", stall.pipelines_made, stall.pipelines_waiting),
            short(stall.sys_pages_in),
        ];
        let c = severity_colour(severity(stall.ms));
        for (i, cell) in cells.iter().enumerate() {
            let colour = match i {
                1 => c,
                4 if stall.sys_pages_in >= PAGING as f32 => RED,
                _ => TEXT,
            };
            p.text(
                Rect::new(inner.x + inner.w * cols[i], y, inner.w * 0.2, row),
                cell,
                SMALL,
                colour,
                Align::Left,
            );
        }
        y += row;
    }
    if !any {
        p.text(
            Rect::new(inner.x, y + 4.0 * s, inner.w, row),
            "\u{2713} NO STALLS YET",
            BODY_BOLD,
            GREEN,
            Align::Left,
        );
    }
}

fn layer_panel(p: &Painter, r: Rect, v: &View) {
    let s = p.scale;
    let inner = p.panel(r, "11", "LAYER // HOLDINGS & SWITCHES", CYAN);
    let n = &v.stats.now;
    let items = [
        (
            "HOST COPIES",
            format!(
                "{} \u{00b7} {}",
                short(n.host_copies),
                size(n.host_copies_mb)
            ),
        ),
        (
            "HOST TEXTURES",
            format!(
                "{} \u{00b7} {}",
                short(n.host_textures),
                size(n.host_textures_mb)
            ),
        ),
        (
            "UPLOAD BUFS",
            format!("{} \u{00b7} {}", short(n.upload_buffers), size(n.upload_mb)),
        ),
        (
            "STAGING / SCRATCH",
            format!("{} / {}", size(n.staging_mb), size(n.scratch_mb)),
        ),
        ("TEX UPLOADS/s", short(n.texture_uploads_per_s)),
        ("ALLOC RENEWED", short(n.allocators_renewed)),
        ("LEFT-OUT LEVELS", short(n.left_out_levels)),
        ("TITLE FRAME", v.stats.title_frame.to_string()),
    ];
    let row = 14.0 * s;
    let half = inner.w / 2.0;
    for (i, (label, value)) in items.iter().enumerate() {
        let x = inner.x + half * (i % 2) as f32;
        let y = inner.y + row * (i / 2) as f32 * 1.0;
        p.text(
            Rect::new(x, y, half * 0.5, row),
            label,
            TINY,
            DIM,
            Align::Left,
        );
        p.text(
            Rect::new(x + half * 0.42, y - 1.0 * s, half * 0.5, row),
            value,
            SMALL,
            TEXT,
            Align::Right,
        );
    }
    let y = inner.y + row * 4.0 + 4.0 * s;
    p.line(inner.x, y, inner.right(), y, CYAN.alpha(0.15), 1.0);
    let switches = v.stats.switches();
    p.text(
        Rect::new(inner.x, y + 3.0 * s, inner.w, inner.bottom() - y - 3.0 * s),
        if switches.is_empty() {
            "(no VICTORY_* switches set)"
        } else {
            switches
        },
        SMALL_WRAP,
        DIM,
        Align::Left,
    );
}
