//! What the overlay works out from a snapshot of the block: the verdict
//! over the last seconds, severities, scales. No drawing, no system calls.

use victory_core::telemetry::{Second, Stats};
use victory_core::verdict::{Cause, Facts, LOW_RAM, PAGING, causes, median, quantile, share};

/// Seconds the bottleneck line judges.
pub const VERDICT_SECONDS: usize = 30;

/// The rules of `victory probe-report` over the last [`VERDICT_SECONDS`]
/// seconds (the menus included: the overlay shows what is happening now).
pub fn verdict(stats: &Stats) -> Vec<Cause> {
    let recent: Vec<&Second> = {
        let all: Vec<&Second> = stats.history().collect();
        all[all.len().saturating_sub(VERDICT_SECONDS)..].to_vec()
    };
    causes(&facts(&recent, stats.threads[0].name()))
}

/// The facts of a stretch of seconds.
pub fn facts(rows: &[&Second], busiest: &str) -> Facts {
    let col = |f: fn(&Second) -> f32| -> Vec<f64> {
        rows.iter()
            .map(|s| f64::from(f(s)))
            .filter(|v| v.is_finite())
            .collect()
    };
    let pages = col(|s| s.sys_pages_in_per_s);
    let ram = col(|s| s.avail_ram_mb);
    let full: Vec<f64> = rows
        .iter()
        .map(|s| {
            if s.vram_budget_mb > 0.0 {
                f64::from(s.vram_used_mb / s.vram_budget_mb)
            } else {
                0.0
            }
        })
        .collect();
    Facts {
        ms_p50: median(&col(|s| s.ms_p50)),
        prep_ms: median(&col(|s| s.prep_ms)),
        gpu_ms: median(&col(|s| s.gpu_ms)),
        dropped: median(&col(|s| s.dropped)),
        waiting_share: share(&col(|s| s.pipelines_waiting), |w| w > 0.0),
        pipeline_ms: median(
            &col(|s| s.pipeline_ms_mean)
                .into_iter()
                .filter(|&v| v > 0.0)
                .collect::<Vec<_>>(),
        ),
        pipeline_wait_ms: median(&col(|s| s.pipeline_wait_ms)),
        pages_in: median(&pages),
        paging_share: share(&pages, |p| p >= PAGING),
        avail_ram_mb: median(&ram),
        low_ram_share: share(&ram, |m| m < LOW_RAM),
        private_mb: quantile(&col(|s| s.private_mb), 0.9),
        ws_mb: median(&col(|s| s.ws_mb)),
        vram_used_mb: median(&col(|s| s.vram_used_mb)),
        vram_budget_mb: median(&col(|s| s.vram_budget_mb)),
        vram_full_share: share(&full, |f| f >= 0.95),
        vram_shared_mb: median(&col(|s| s.vram_shared_mb)),
        busiest_thread: busiest.to_owned(),
        busiest_cores: median(&col(|s| s.busiest_thread_cores)),
    }
}

/// How bad a frame time is: 0 on time (33 ms), 1 late (50), 2 slow (100), 3 a stall.
pub fn severity(ms: f32) -> u8 {
    match ms {
        m if m <= 33.4 => 0,
        m if m <= 50.0 => 1,
        m if m <= 100.0 => 2,
        _ => 3,
    }
}

/// The top of a frame-time graph: the smallest of a few steps over `max`.
pub fn graph_top(max: f32) -> f32 {
    const STEPS: [f32; 8] = [50.0, 100.0, 150.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0];
    STEPS
        .iter()
        .copied()
        .find(|&s| s >= max * 1.05)
        .unwrap_or(8000.0)
}

/// The top of a sparkline: a round number at or over `max` (never 0).
pub fn nice_top(max: f32) -> f32 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let p = 10f32.powf(max.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if m * p >= max {
            return m * p;
        }
    }
    10.0 * p
}

/// A count in few characters: 950, 9.3k, 12k, 1.2M.
pub fn short(v: f32) -> String {
    let a = v.abs();
    if !v.is_finite() {
        "-".into()
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}k", v / 1e3)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

/// MB as GB with one decimal past 1 GB.
pub fn size(mb: f32) -> String {
    if mb >= 1024.0 {
        format!("{:.1}G", mb / 1024.0)
    } else {
        format!("{mb:.0}M")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_core::verdict::Kind;

    fn second(ms: f32) -> Second {
        Second {
            ms_p50: ms,
            prep_ms: 8.0,
            gpu_ms: 10.0,
            avail_ram_mb: 6000.0,
            vram_budget_mb: 10000.0,
            vram_used_mb: 6000.0,
            sys_pages_in_per_s: 50.0,
            ..Second::ZERO
        }
    }

    #[test]
    fn verdict_follows_the_last_seconds() {
        let mut stats = Box::new(Stats::ZERO);
        // Long ago: paging hard. Lately: calm. Only the last 30 s count.
        for _ in 0..60 {
            stats.push_second(Second {
                sys_pages_in_per_s: 9000.0,
                avail_ram_mb: 300.0,
                ..second(400.0)
            });
        }
        for _ in 0..VERDICT_SECONDS {
            stats.push_second(second(33.0));
        }
        assert_eq!(verdict(&stats)[0].kind, Kind::Nothing);
        for _ in 0..12 {
            stats.push_second(Second {
                sys_pages_in_per_s: 4000.0,
                ..second(300.0)
            });
        }
        assert_eq!(
            verdict(&stats)[0].kind,
            Kind::Ram,
            "12 of 30 seconds paging: over a quarter"
        );
    }

    #[test]
    fn verdict_on_an_empty_block_is_calm() {
        let stats = Box::new(Stats::ZERO);
        assert_eq!(verdict(&stats)[0].kind, Kind::Nothing);
    }

    #[test]
    fn card_memory_share() {
        let rows: Vec<Second> = (0..10)
            .map(|_| Second {
                vram_used_mb: 9800.0,
                ..second(40.0)
            })
            .collect();
        let refs: Vec<&Second> = rows.iter().collect();
        let f = facts(&refs, "victory-submit");
        assert_eq!(f.vram_full_share, 1.0);
        assert_eq!(causes(&f)[0].kind, Kind::CardMemory);
        assert_eq!(f.busiest_thread, "victory-submit");
    }

    #[test]
    fn severities_and_scales() {
        assert_eq!(
            [16.0, 33.4, 34.0, 50.0, 99.0, 101.0].map(severity),
            [0, 0, 1, 1, 2, 3]
        );
        assert_eq!(graph_top(30.0), 50.0);
        assert_eq!(graph_top(48.0), 100.0);
        assert_eq!(graph_top(380.0), 500.0);
        assert_eq!(graph_top(1e9), 8000.0);
        assert_eq!(nice_top(0.0), 1.0);
        assert_eq!(nice_top(7.0), 10.0);
        assert_eq!(nice_top(1800.0), 2000.0);
        assert_eq!(nice_top(2100.0), 2500.0);
        assert_eq!(short(950.0), "950");
        assert_eq!(short(9300.0), "9.3k");
        assert_eq!(short(12_400.0), "12k");
        assert_eq!(size(512.0), "512M");
        assert_eq!(size(2048.0), "2.0G");
    }
}
