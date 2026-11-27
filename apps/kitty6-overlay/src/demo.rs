//! `--demo`: a stand-in for the game that publishes made-up but plausible
//! numbers into a block of its own name (`DEMO_NAME`, never the game's), so
//! the overlay can be tried and screenshotted without the game.

use victory_core::telemetry::{Block, Second, Stall, Thread};

/// A deterministic run: calm stretches, a paging storm every minute, early
/// pipeline building.
pub struct Sim {
    rng: u64,
    /// Seconds simulated.
    pub t: f64,
    frames_this_second: Vec<(f32, u32)>,
}

impl Default for Sim {
    fn default() -> Self {
        Sim::new()
    }
}

impl Sim {
    pub fn new() -> Sim {
        Sim {
            rng: 0x9e37_79b9_7f4a_7c15,
            t: 0.0,
            frames_this_second: Vec::new(),
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / (1u64 << 24) as f32
    }

    /// 0 to 1: how hard the system pages now.
    fn storm(&self) -> f32 {
        let phase = self.t % 60.0;
        if (32.0..47.0).contains(&phase) {
            (((phase - 32.0) / 15.0) * std::f64::consts::PI).sin() as f32
        } else {
            0.0
        }
    }

    fn building(&self) -> f32 {
        (1.0 - self.t as f32 / 40.0).max(0.0)
    }

    /// The next frame: its time and page faults.
    pub fn frame(&mut self) -> (f32, u32) {
        let storm = self.storm();
        let base = 30.0 + 6.0 * (self.t as f32 / 9.0).sin() + 4.0 * self.rand();
        let mut ms = base + storm * 70.0 * self.rand() + self.building() * 25.0 * self.rand();
        if self.rand() < 0.004 + storm * 0.05 {
            ms += 90.0 + 300.0 * self.rand();
        }
        let faults = (200.0 + 4000.0 * storm * self.rand() + 50.0 * self.rand()) as u32;
        self.t += f64::from(ms) / 1000.0;
        self.frames_this_second.push((ms, faults));
        (ms, faults)
    }

    /// Whether a second's worth of frames has gone by since the last
    /// [`Sim::second`].
    pub fn second_due(&self) -> bool {
        self.frames_this_second.iter().map(|f| f.0).sum::<f32>() >= 1000.0
    }

    /// The second just ended, and its stall.
    pub fn second(&mut self) -> (Second, Option<Stall>) {
        let frames = std::mem::take(&mut self.frames_this_second);
        let mut ms: Vec<f32> = frames.iter().map(|f| f.0).collect();
        ms.sort_by(f32::total_cmp);
        let n = ms.len().max(1);
        let at = |q: f32| {
            ms.get(((q * n as f32).ceil() as usize).clamp(1, n) - 1)
                .copied()
                .unwrap_or(0.0)
        };
        let mean = ms.iter().sum::<f32>() / n as f32;
        let storm = self.storm();
        let building = self.building();
        let r = self.rand();
        let waiting = (building * 420.0 * (0.8 + 0.4 * r)).round();
        let total_ram = 32_604.0;
        let avail = 2600.0 - 2100.0 * storm + 300.0 * r;
        let worst = frames
            .iter()
            .copied()
            .fold((0.0f32, 0u32), |a, f| if f.0 > a.0 { f } else { a });
        let second = Second {
            t_s: self.t as f32,
            frames: n as f32,
            fps: n as f32 * 1000.0 / ms.iter().sum::<f32>().max(1.0),
            ms_mean: mean,
            ms_p50: at(0.5),
            ms_p95: at(0.95),
            ms_max: at(1.0),
            over33: ms.iter().filter(|&&m| m > 33.4).count() as f32,
            over100: ms.iter().filter(|&&m| m > 100.0).count() as f32,
            over500: ms.iter().filter(|&&m| m > 500.0).count() as f32,
            prep_ms: (mean * (0.42 + 0.1 * r)).min(mean),
            gpu_ms: mean * (0.38 - 0.2 * storm),
            draws: 2100.0 + 500.0 * (self.t as f32 / 13.0).sin() + 100.0 * r,
            dropped: building * 900.0 * r,
            pipelines_waiting: waiting,
            pipelines_made: if building > 0.0 {
                18.0 + 10.0 * r
            } else {
                (r * 1.4).floor()
            },
            pipeline_ms_mean: 38.0 + 12.0 * r,
            pipeline_ms_max: 120.0 + 80.0 * r,
            pipeline_wait_ms: building * 60.0 * r,
            pipelines_refused: 6.0,
            ws_mb: 9400.0 - 3800.0 * storm,
            private_mb: 21_900.0 + 400.0 * r,
            faults_per_s: 9000.0 + 60_000.0 * storm * r,
            io_read_mb_s: 18.0 + 60.0 * storm * r + 30.0 * self.rand(),
            sys_pages_in_per_s: 120.0 + 9500.0 * storm * (0.6 + 0.4 * r),
            avail_ram_mb: avail,
            total_ram_mb: total_ram,
            avail_commit_mb: 9_800.0 - 1200.0 * storm,
            commit_limit_mb: 53_000.0,
            mem_load_pct: 100.0 * (1.0 - avail / total_ram),
            vram_budget_mb: 10_650.0,
            vram_used_mb: 9_300.0 + 900.0 * storm + 150.0 * r,
            vram_shared_mb: 640.0 + 700.0 * storm,
            cpu_cores: 4.6 + 1.2 * r,
            busiest_thread_cores: 0.78 + 0.18 * r,
            host_copies: 18_400.0 + 300.0 * r,
            host_copies_mb: 3_120.0,
            host_textures: 41_200.0,
            host_textures_mb: 4_480.0,
            staging_mb: 96.0 + 40.0 * r,
            upload_buffers: 212.0,
            upload_mb: 288.0,
            scratch_mb: 64.0,
            texture_uploads_per_s: 140.0 + 400.0 * storm * r,
            allocators_renewed: (self.t / 8.0).floor() as f32,
            left_out_levels: 1_204.0,
        };
        let stall = (worst.0 >= 100.0).then(|| Stall {
            t_s: self.t as f32,
            ms: worst.0,
            faults: worst.1 as f32,
            pipelines_made: if building > 0.0 {
                (3.0 * r).round()
            } else {
                0.0
            },
            pipelines_waiting: waiting,
            sys_pages_in: second.sys_pages_in_per_s,
        });
        (second, stall)
    }
}

/// The parts of the block a game fills once.
pub fn announce(block: &Block, hwnd: usize) {
    block.announce(std::process::id());
    block
        .header
        .hwnd
        .store(hwnd as u64, std::sync::atomic::Ordering::Relaxed);
    block.stats.write(|s| {
        s.cpu_logical = 28;
        s.width = 2560;
        s.height = 1440;
        s.stall_ms = 100.0;
        victory_core::telemetry::put_text(
            &mut s.switches,
            "CARD_BUFFERS_MB=1536 CARD_WINDOWS_MB=1024 CARD_STEPS=2 INDIRECT_BATCH=2 JOIN_COPIES=1 JOIN_PARTS=8 \
             LOG=warn RELAY=2 RESIDENCY=1 TEXTURE_MIP_BIAS=1 TEXTURE_NO_WAIT=1 TEXTURE_ROOM_PERCENT=80 THIN=1 \
             UPLOAD_AHEAD=1 XMA_REFILL=1",
        );
        let threads = [
            ("victory-submit", 0.93),
            ("victory-thin-3", 0.61),
            ("title-render-0", 0.55),
            ("victory-pipeline-1", 0.42),
            ("title-stream", 0.31),
        ];
        for (slot, (name, cores)) in s.threads.iter_mut().zip(threads) {
            *slot = Thread::new(name, cores);
        }
        let kinds = [("victory-thin-#", 1.9), ("title-render-#", 1.2), ("victory-submit", 0.93), ("victory-pipeline-#", 0.8)];
        for (slot, (name, cores)) in s.groups.iter_mut().zip(kinds) {
            *slot = Thread::new(name, cores);
        }
    });
}

/// Runs the simulation for `seconds` at once into `block`.
pub fn fill(block: &Block, sim: &mut Sim, seconds: f64) {
    let until = sim.t + seconds;
    while sim.t < until {
        step(block, sim);
    }
}

/// One frame into the block (and the second, when due); the frame's ms.
pub fn step(block: &Block, sim: &mut Sim) -> f32 {
    let (ms, faults) = sim.frame();
    block.frames.write(|f| f.push(ms, faults));
    if sim.second_due() {
        let (second, stall) = sim.second();
        let frame = (sim.t * 33.0) as u64;
        block.stats.write(|s| {
            s.push_second(second);
            if let Some(stall) = stall {
                s.push_stall(stall);
            }
            s.title_frame = 81_000 + frame;
        });
        block
            .header
            .game_tick_ms
            .store(crate::link::now_ms(), std::sync::atomic::Ordering::Relaxed);
    }
    ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_pages_and_builds_pipelines() {
        let mut sim = Sim::new();
        let mut seconds = Vec::new();
        let mut stalls = 0;
        while sim.t < 120.0 {
            sim.frame();
            if sim.second_due() {
                let (s, stall) = sim.second();
                stalls += usize::from(stall.is_some());
                seconds.push(s);
            }
        }
        assert!(seconds.len() > 100);
        assert!(
            seconds[..10].iter().all(|s| s.pipelines_waiting > 0.0),
            "pipelines early"
        );
        assert!(
            seconds.iter().any(|s| s.sys_pages_in_per_s > 1500.0),
            "a paging storm"
        );
        assert!(seconds.iter().all(|s| s.fps > 1.0 && s.ms_p50 > 0.0));
        assert!(stalls > 0);
    }
}
