//! What a slow PC is short of: the rules `victory probe-report` and the
//! Kitty6 overlay both judge a stretch of seconds by (docs/probes.md).

/// Paging this heavy (pages read in a second, the whole system) slows
/// everything; a few hundred is normal.
pub const PAGING: f64 = 1500.0;
/// Free RAM under this (MB) and Windows trims working sets.
pub const LOW_RAM: f64 = 1024.0;

/// The numbers the rules look at, over a stretch of seconds.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// Median frame time, ms.
    pub ms_p50: f64,
    pub prep_ms: f64,
    pub gpu_ms: f64,
    /// Draws a frame dropped for want of a host pipeline (median).
    pub dropped: f64,
    /// Share of the seconds with host pipelines waiting.
    pub waiting_share: f64,
    pub pipeline_ms: f64,
    pub pipeline_wait_ms: f64,
    /// Median system pages read in a second.
    pub pages_in: f64,
    /// Share of the seconds paging at [`PAGING`] or more.
    pub paging_share: f64,
    pub avail_ram_mb: f64,
    /// Share of the seconds with free RAM under [`LOW_RAM`].
    pub low_ram_share: f64,
    pub private_mb: f64,
    pub ws_mb: f64,
    pub vram_used_mb: f64,
    pub vram_budget_mb: f64,
    /// Share of the seconds with the card's memory at 95 % of the budget.
    pub vram_full_share: f64,
    pub vram_shared_mb: f64,
    pub busiest_thread: String,
    pub busiest_cores: f64,
}

/// What a cause is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ram,
    CardMemory,
    Pipelines,
    Preparing,
    Gpu,
    Nothing,
}

impl Kind {
    /// A few words for a small display.
    pub fn short(self) -> &'static str {
        match self {
            Kind::Ram => "RAM / PAGING",
            Kind::CardMemory => "CARD MEMORY",
            Kind::Pipelines => "PIPELINES",
            Kind::Preparing => "CPU PREP",
            Kind::Gpu => "GPU",
            Kind::Nothing => "NOTHING STANDS OUT",
        }
    }
}

/// A likely cause with its evidence.
#[derive(Clone, Debug)]
pub struct Cause {
    pub kind: Kind,
    pub score: f64,
    pub text: String,
}

/// The likeliest causes first.
pub fn causes(s: &Facts) -> Vec<Cause> {
    let mut out: Vec<Cause> = Vec::new();
    let mut push = |kind, score, text| out.push(Cause { kind, score, text });
    let frame = s.ms_p50.max(1.0);
    if s.paging_share > 0.25 || s.low_ram_share > 0.4 {
        push(
            Kind::Ram,
            3.0 + s.paging_share,
            format!(
                "RAM: the system is paging. {:.0}% of the seconds read {PAGING:.0}+ pages a second from disk (median {:.0}), free RAM was under {LOW_RAM:.0} MB in {:.0}% of them (median {:.0} MB) while the process held {:.0} MB private with only {:.0} MB in RAM. Close other programs, add RAM, or try VICTORY_RESIDENCY=1.",
                s.paging_share * 100.0,
                s.pages_in,
                s.low_ram_share * 100.0,
                s.avail_ram_mb,
                s.private_mb,
                s.ws_mb
            ),
        );
    }
    if s.vram_full_share > 0.3 || s.vram_shared_mb > 1024.0 {
        push(
            Kind::CardMemory,
            2.5 + s.vram_full_share,
            format!(
                "Card memory: in use at 95%+ of the system's budget in {:.0}% of the seconds (median {:.0} of {:.0} MB), {:.0} MB of the PC's memory used by the card instead. Try VICTORY_LEVELS_ON_DEMAND=1.",
                s.vram_full_share * 100.0,
                s.vram_used_mb,
                s.vram_budget_mb,
                s.vram_shared_mb
            ),
        );
    }
    if s.waiting_share > 0.3 && (s.dropped > 50.0 || s.pipeline_wait_ms > 0.1 * frame) {
        push(
            Kind::Pipelines,
            2.0 + s.waiting_share,
            format!(
                "Pipelines: host pipelines were still being made in {:.0}% of the seconds ({:.0} draws a frame dropped for want of one, {:.0} ms a second waited, a pipeline takes {:.0} ms). A run's first minutes; run `victory warm --used` (with VICTORY_PIPELINE_BOOK=1 in the runs before).",
                s.waiting_share * 100.0,
                s.dropped,
                s.pipeline_wait_ms,
                s.pipeline_ms
            ),
        );
    }
    if s.prep_ms > 0.7 * frame {
        push(
            Kind::Preparing,
            1.5 + s.prep_ms / frame,
            format!(
                "Preparing submissions: {:.0} of the {:.0} ms of a frame (median), the busiest thread `{}` at {:.2} cores{}.",
                s.prep_ms,
                frame,
                s.busiest_thread,
                s.busiest_cores,
                if s.busiest_cores > 0.9 {
                    ": one thread is the wall"
                } else {
                    ""
                }
            ),
        );
    }
    if s.gpu_ms > 0.7 * frame {
        push(
            Kind::Gpu,
            1.4 + s.gpu_ms / frame,
            format!(
                "The GPU: {:.0} of the {:.0} ms of a frame busy (median).",
                s.gpu_ms, frame
            ),
        );
    }
    if out.is_empty() {
        out.push(Cause {
            kind: Kind::Nothing,
            score: 0.0,
            text: format!(
                "Nothing stands out: {:.0} ms a frame (median), {:.0} preparing, {:.0} on the GPU.",
                frame, s.prep_ms, s.gpu_ms
            ),
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// The value at fraction `q` of `v` (nearest rank), NaN when empty.
pub fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let rank = (q * s.len() as f64).ceil() as usize;
    s[rank.clamp(1, s.len()) - 1]
}

pub fn median(v: &[f64]) -> f64 {
    quantile(v, 0.5)
}

/// The share of `v` for which `f` holds (0 when empty).
pub fn share(v: &[f64], f: impl Fn(f64) -> bool) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().filter(|&&x| f(x)).count() as f64 / v.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calm() -> Facts {
        Facts {
            ms_p50: 33.0,
            prep_ms: 10.0,
            gpu_ms: 12.0,
            avail_ram_mb: 8000.0,
            vram_budget_mb: 10000.0,
            ..Facts::default()
        }
    }

    #[test]
    fn nothing_when_calm() {
        let c = causes(&calm());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, Kind::Nothing);
        assert!(c[0].text.starts_with("Nothing stands out: 33 ms"));
    }

    #[test]
    fn paging_beats_everything() {
        let s = Facts {
            paging_share: 0.5,
            prep_ms: 30.0,
            gpu_ms: 30.0,
            vram_full_share: 0.9,
            ..calm()
        };
        let c = causes(&s);
        let kinds: Vec<Kind> = c.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![Kind::Ram, Kind::CardMemory, Kind::Preparing, Kind::Gpu]
        );
        assert!(c[0].text.starts_with("RAM: the system is paging. 50%"));
    }

    #[test]
    fn low_ram_alone_counts_as_ram() {
        let s = Facts {
            low_ram_share: 0.41,
            ..calm()
        };
        assert_eq!(causes(&s)[0].kind, Kind::Ram);
        let s = Facts {
            low_ram_share: 0.39,
            paging_share: 0.25,
            ..calm()
        };
        assert_eq!(
            causes(&s)[0].kind,
            Kind::Nothing,
            "both at the edge: not yet"
        );
    }

    #[test]
    fn card_memory_by_share_or_spill() {
        assert_eq!(
            causes(&Facts {
                vram_full_share: 0.31,
                ..calm()
            })[0]
                .kind,
            Kind::CardMemory
        );
        assert_eq!(
            causes(&Facts {
                vram_shared_mb: 1100.0,
                ..calm()
            })[0]
                .kind,
            Kind::CardMemory
        );
    }

    #[test]
    fn pipelines_need_harm_too() {
        assert_eq!(
            causes(&Facts {
                waiting_share: 0.9,
                ..calm()
            })[0]
                .kind,
            Kind::Nothing,
            "waiting but nothing dropped"
        );
        assert_eq!(
            causes(&Facts {
                waiting_share: 0.9,
                dropped: 51.0,
                ..calm()
            })[0]
                .kind,
            Kind::Pipelines
        );
        assert_eq!(
            causes(&Facts {
                waiting_share: 0.9,
                pipeline_wait_ms: 4.0,
                ..calm()
            })[0]
                .kind,
            Kind::Pipelines
        );
    }

    #[test]
    fn prep_and_gpu_against_the_frame() {
        let s = Facts {
            prep_ms: 30.0,
            busiest_cores: 0.95,
            busiest_thread: "victory-submit".into(),
            ..calm()
        };
        let c = causes(&s);
        assert_eq!(c[0].kind, Kind::Preparing);
        assert!(
            c[0].text.ends_with("one thread is the wall."),
            "{}",
            c[0].text
        );
        assert_eq!(
            causes(&Facts {
                gpu_ms: 30.0,
                ..calm()
            })[0]
                .kind,
            Kind::Gpu
        );
        // Equal shares: preparing ranks above the GPU.
        let c = causes(&Facts {
            gpu_ms: 30.0,
            prep_ms: 30.0,
            ..calm()
        });
        assert_eq!((c[0].kind, c[1].kind), (Kind::Preparing, Kind::Gpu));
    }

    #[test]
    fn stats() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert!(median(&[]).is_nan());
        assert_eq!(share(&[1.0, 2.0, 3.0, 4.0], |x| x > 2.0), 0.5);
        assert_eq!(share(&[], |_| true), 0.0);
    }
}
