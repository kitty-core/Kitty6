//! `victory probe-report <csv> [<csv>]`: what a run's probe file
//! (`VICTORY_PROBE`, `victory_gpu`'s `probe` module) says the PC was short
//! of. The seconds in the world (draws at or over `--world-draws`) are read
//! apart from the menus; with two files the second is compared with the
//! first.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
pub use victory_core::verdict::{median, quantile};
use victory_core::verdict::{Facts, LOW_RAM, PAGING, causes, share};

/// A probe file: its columns by name, and its rows (a cell that is no
/// number is NaN; the thread name column is kept as text).
pub struct Table {
    names: HashMap<String, usize>,
    rows: Vec<Vec<f64>>,
    threads: Vec<String>,
}

impl Table {
    pub fn parse(text: &str) -> Result<Table> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines.next().context("an empty probe file")?;
        let names: HashMap<String, usize> =
            header.split(',').enumerate().map(|(i, n)| (n.trim().to_owned(), i)).collect();
        for needed in ["t_s", "frames", "ms_mean", "draws"] {
            if !names.contains_key(needed) {
                bail!("not a probe file: no `{needed}` column");
            }
        }
        let thread_col = names.get("busiest_thread").copied();
        let mut rows = Vec::new();
        let mut threads = Vec::new();
        for line in lines {
            let cells: Vec<&str> = line.split(',').collect();
            if cells.len() != names.len() {
                continue; // a row cut off when the run ended
            }
            rows.push(cells.iter().map(|c| c.trim().parse().unwrap_or(f64::NAN)).collect());
            threads.push(thread_col.map(|i| cells[i].to_owned()).unwrap_or_default());
        }
        Ok(Table { names, rows, threads })
    }

    /// A column's values over `rows` (indices); NaN cells left out.
    fn col(&self, name: &str, rows: &[usize]) -> Vec<f64> {
        let Some(&i) = self.names.get(name) else { return Vec::new() };
        rows.iter().map(|&r| self.rows[r][i]).filter(|v| v.is_finite()).collect()
    }

    /// Pairs of two columns over `rows`, where both are numbers.
    fn pairs(&self, a: &str, b: &str, rows: &[usize]) -> Vec<(f64, f64)> {
        let (Some(&i), Some(&j)) = (self.names.get(a), self.names.get(b)) else { return Vec::new() };
        rows.iter()
            .map(|&r| (self.rows[r][i], self.rows[r][j]))
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .collect()
    }

    fn get(&self, row: usize, name: &str) -> f64 {
        self.names.get(name).map_or(f64::NAN, |&i| self.rows[row][i])
    }
}


/// Pearson's correlation of the pairs; NaN with fewer than 3 or no spread.
pub fn correlation(p: &[(f64, f64)]) -> f64 {
    if p.len() < 3 {
        return f64::NAN;
    }
    let n = p.len() as f64;
    let (mx, my) = (p.iter().map(|q| q.0).sum::<f64>() / n, p.iter().map(|q| q.1).sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for &(x, y) in p {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    if sxx == 0.0 || syy == 0.0 { f64::NAN } else { sxy / (sxx * syy).sqrt() }
}


/// What a stretch of a run measured, the numbers the verdict is made of.
#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub seconds: usize,
    pub frames: f64,
    pub fps: f64,
    pub ms_p50: f64,
    pub ms_p95: f64,
    pub ms_worst: f64,
    pub stalls_over500: f64,
    pub prep_ms: f64,
    pub gpu_ms: f64,
    pub dropped: f64,
    pub waiting_share: f64,
    pub pipeline_ms: f64,
    pub pipeline_wait_ms: f64,
    pub pages_in: f64,
    pub paging_share: f64,
    pub avail_ram_mb: f64,
    pub low_ram_share: f64,
    pub avail_commit_mb: f64,
    pub private_mb: f64,
    pub ws_mb: f64,
    pub vram_used_mb: f64,
    pub vram_budget_mb: f64,
    pub vram_full_share: f64,
    pub vram_shared_mb: f64,
    pub cpu_cores: f64,
    pub busiest_cores: f64,
    pub busiest_thread: String,
    pub io_read_mb_s: f64,
    pub corr: Vec<(&'static str, f64)>,
}


pub fn summarize(t: &Table, rows: &[usize]) -> Summary {
    let frames = t.col("frames", rows);
    let total_frames: f64 = frames.iter().sum();
    let pages = t.col("sys_pages_in_per_s", rows);
    let ram = t.col("avail_ram_mb", rows);
    let used = t.col("vram_used_mb", rows);
    let budget = t.col("vram_budget_mb", rows);
    let full: Vec<f64> = t
        .pairs("vram_used_mb", "vram_budget_mb", rows)
        .into_iter()
        .map(|(u, b)| if b > 0.0 { u / b } else { 0.0 })
        .collect();
    // Seconds the rows cover: a row is a second or, at seconds per frame,
    // longer (its frames over its rate).
    let elapsed: f64 = t
        .pairs("frames", "fps", rows)
        .into_iter()
        .map(|(f, r)| if r > 0.0 { f / r } else { 1.0 })
        .sum::<f64>()
        .max(1e-9);
    let mut busiest: HashMap<&str, usize> = HashMap::new();
    for &r in rows {
        *busiest.entry(t.threads[r].as_str()).or_default() += 1;
    }
    let busiest_thread = busiest.into_iter().max_by_key(|e| e.1).map(|e| e.0.to_owned()).unwrap_or_default();
    let corr = [
        ("system paging (pages in/s)", "sys_pages_in_per_s"),
        ("process page faults/s", "faults_per_s"),
        ("free RAM", "avail_ram_mb"),
        ("card memory in use", "vram_used_mb"),
        ("pipelines waiting", "pipelines_waiting"),
        ("ms preparing", "prep_ms"),
        ("ms on the GPU", "gpu_ms"),
        ("draws", "draws"),
        ("process disk reads MB/s", "io_read_mb_s"),
    ]
    .into_iter()
    .map(|(label, col)| (label, correlation(&t.pairs(col, "ms_mean", rows))))
    .collect();
    Summary {
        seconds: rows.len(),
        frames: total_frames,
        fps: total_frames / elapsed,
        ms_p50: median(&t.col("ms_p50", rows)),
        ms_p95: quantile(&t.col("ms_p95", rows), 0.5),
        ms_worst: t.col("ms_max", rows).into_iter().fold(f64::NAN, f64::max),
        stalls_over500: t.col("over500", rows).iter().sum(),
        prep_ms: median(&t.col("prep_ms", rows)),
        gpu_ms: median(&t.col("gpu_ms", rows)),
        dropped: median(&t.col("dropped", rows)),
        waiting_share: share(&t.col("pipelines_waiting", rows), |w| w > 0.0),
        pipeline_ms: median(&t.col("pipeline_ms_mean", rows).into_iter().filter(|&v| v > 0.0).collect::<Vec<_>>()),
        pipeline_wait_ms: median(&t.col("pipeline_wait_ms", rows)),
        pages_in: median(&pages),
        paging_share: share(&pages, |p| p >= PAGING),
        avail_ram_mb: median(&ram),
        low_ram_share: share(&ram, |m| m < LOW_RAM),
        avail_commit_mb: median(&t.col("avail_commit_mb", rows)),
        private_mb: quantile(&t.col("private_mb", rows), 0.9),
        ws_mb: median(&t.col("ws_mb", rows)),
        vram_used_mb: median(&used),
        vram_budget_mb: median(&budget),
        vram_full_share: share(&full, |f| f >= 0.95),
        vram_shared_mb: median(&t.col("vram_shared_mb", rows)),
        cpu_cores: median(&t.col("cpu_cores", rows)),
        busiest_cores: median(&t.col("busiest_thread_cores", rows)),
        busiest_thread,
        io_read_mb_s: median(&t.col("io_read_mb_s", rows)),
        corr,
    }
}

/// What the PC was short of, the likeliest first, each with its evidence
/// (the rules are `victory_core::verdict`, shared with the Kitty6 overlay).
pub fn verdict(s: &Summary) -> Vec<String> {
    let facts = Facts {
        ms_p50: s.ms_p50,
        prep_ms: s.prep_ms,
        gpu_ms: s.gpu_ms,
        dropped: s.dropped,
        waiting_share: s.waiting_share,
        pipeline_ms: s.pipeline_ms,
        pipeline_wait_ms: s.pipeline_wait_ms,
        pages_in: s.pages_in,
        paging_share: s.paging_share,
        avail_ram_mb: s.avail_ram_mb,
        low_ram_share: s.low_ram_share,
        private_mb: s.private_mb,
        ws_mb: s.ws_mb,
        vram_used_mb: s.vram_used_mb,
        vram_budget_mb: s.vram_budget_mb,
        vram_full_share: s.vram_full_share,
        vram_shared_mb: s.vram_shared_mb,
        busiest_thread: s.busiest_thread.clone(),
        busiest_cores: s.busiest_cores,
    };
    causes(&facts).into_iter().map(|c| c.text).collect()
}

fn print_summary(title: &str, s: &Summary) {
    println!("{title}: {} s, {:.0} frames ({:.2} fps)", s.seconds, s.frames, s.fps);
    println!(
        "  frame ms: median {:.1}, 95th {:.1}, worst {:.0}; {:.0} frames over 500 ms",
        s.ms_p50, s.ms_p95, s.ms_worst, s.stalls_over500
    );
    println!(
        "  per frame: {:.1} ms preparing, {:.1} ms on the GPU, {:.0} draws dropped without a pipeline",
        s.prep_ms, s.gpu_ms, s.dropped
    );
    println!(
        "  pipelines: waiting in {:.0}% of seconds, {:.0} ms to make one, {:.0} ms/s waited by first uses",
        s.waiting_share * 100.0,
        s.pipeline_ms,
        s.pipeline_wait_ms
    );
    println!(
        "  RAM: process {:.0} MB private (90th), {:.0} MB in RAM; system {:.0} MB free, {:.0} MB commit free, {:.0} pages in/s ({:.0}% of seconds paging hard)",
        s.private_mb,
        s.ws_mb,
        s.avail_ram_mb,
        s.avail_commit_mb,
        s.pages_in,
        s.paging_share * 100.0
    );
    println!(
        "  card: {:.0} of {:.0} MB budget ({:.0}% of seconds at 95%+), {:.0} MB in the PC's memory",
        s.vram_used_mb,
        s.vram_budget_mb,
        s.vram_full_share * 100.0,
        s.vram_shared_mb
    );
    println!(
        "  CPU: {:.1} cores busy, busiest thread `{}` {:.2} cores; disk reads {:.0} MB/s",
        s.cpu_cores, s.busiest_thread, s.busiest_cores, s.io_read_mb_s
    );
    let mut corr: Vec<&(&str, f64)> = s.corr.iter().filter(|c| c.1.is_finite()).collect();
    corr.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    let text: Vec<String> = corr.iter().take(5).map(|(l, c)| format!("{l} {c:+.2}")).collect();
    if !text.is_empty() {
        println!("  frame time moves with: {}", text.join(", "));
    }
    println!("  likeliest causes:");
    for (i, v) in verdict(s).iter().enumerate() {
        println!("    {}. {v}", i + 1);
    }
}

/// The rows in the world: draws at or over `world_draws`.
fn world_rows(t: &Table, world_draws: f64) -> Vec<usize> {
    (0..t.rows.len()).filter(|&r| t.get(r, "draws") >= world_draws).collect()
}

fn change(name: &str, a: f64, b: f64, lower_is_better: bool) {
    let pct = if a.abs() > 1e-9 { (b - a) / a * 100.0 } else { f64::NAN };
    let better = if lower_is_better { b < a } else { b > a };
    let mark = if !pct.is_finite() || pct.abs() < 5.0 {
        "~"
    } else if better {
        "better"
    } else {
        "worse"
    };
    println!("  {name:<28} {a:>10.1} -> {b:>10.1}  ({pct:+.0}%, {mark})");
}

pub fn run(files: Vec<std::path::PathBuf>, world_draws: f64) -> Result<()> {
    if files.is_empty() || files.len() > 2 {
        bail!("give one probe file, or two to compare");
    }
    let read = |p: &Path| -> Result<Table> {
        Table::parse(&std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?)
    };
    let tables: Vec<Table> = files.iter().map(|p| read(p)).collect::<Result<_>>()?;
    let mut worlds = Vec::new();
    for (path, t) in files.iter().zip(&tables) {
        println!("== {}", path.display());
        let all: Vec<usize> = (0..t.rows.len()).collect();
        let world = world_rows(t, world_draws);
        print_summary("whole run", &summarize(t, &all));
        if world.is_empty() {
            println!("in the world: no second with {world_draws:.0}+ draws");
            worlds.push(None);
        } else {
            let s = summarize(t, &world);
            print_summary(&format!("in the world ({world_draws:.0}+ draws a frame)"), &s);
            worlds.push(Some(s));
        }
    }
    if let [Some(a), Some(b)] = worlds.as_slice() {
        println!("== in the world, second file against the first");
        change("fps", a.fps, b.fps, false);
        change("frame ms, median", a.ms_p50, b.ms_p50, true);
        change("frame ms, 95th", a.ms_p95, b.ms_p95, true);
        change("ms preparing", a.prep_ms, b.prep_ms, true);
        change("ms on the GPU", a.gpu_ms, b.gpu_ms, true);
        change("pages in/s", a.pages_in, b.pages_in, true);
        change("free RAM MB", a.avail_ram_mb, b.avail_ram_mb, false);
        change("private MB", a.private_mb, b.private_mb, true);
        change("card MB in use", a.vram_used_mb, b.vram_used_mb, true);
        change("draws dropped", a.dropped, b.dropped, true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "t_s,frames,fps,ms_mean,ms_p50,ms_p95,ms_max,over33,over100,over500,stall_ms,stall_faults,stall_pipelines_made,prep_ms,gpu_ms,draws,dropped,pipelines_waiting,pipelines_made,pipeline_ms_mean,pipeline_ms_max,pipeline_wait_ms,ws_mb,private_mb,faults_per_s,io_read_mb_s,sys_pages_in_per_s,avail_ram_mb,avail_commit_mb,mem_load_pct,vram_budget_mb,vram_used_mb,vram_shared_mb,cpu_cores,busiest_thread,busiest_thread_cores";

    fn row(ms: f64, draws: f64, pages: f64, ram: f64, waiting: f64, prep: f64) -> String {
        format!(
            "1,{:.0},1,{ms},{ms},{ms},{ms},1,1,0,0,0,0,{prep},10,{draws},0,{waiting},0,0,0,0,5000,22000,100,5,{pages},{ram},8000,95,11000,9000,100,3,victory-submit,0.5",
            1000.0 / ms
        )
    }

    #[test]
    fn paging_run_is_called_paging() {
        let mut text = String::from(HEAD);
        for i in 0..20 {
            text.push('\n');
            text.push_str(&row(600.0 + i as f64 * 20.0, 2000.0, 9000.0 + i as f64 * 100.0, 600.0, 0.0, 100.0));
        }
        text.push_str("\n1,30,30,33,33,33,33,0,0,0,0,0,0,1,1,20,0,0,0,0,0,0,1000,2000,10,1,10,20000,30000,40,11000,500,0,1,x,0.1"); // a menu second
        let t = Table::parse(&text).unwrap();
        assert_eq!(t.rows.len(), 21);
        let world = world_rows(&t, 500.0);
        assert_eq!(world.len(), 20);
        let s = summarize(&t, &world);
        assert!(s.paging_share > 0.99);
        let v = verdict(&s);
        assert!(v[0].starts_with("RAM"), "{v:?}");
        // Frame time rises with paging in these rows.
        let c = s.corr.iter().find(|c| c.0.starts_with("system paging")).unwrap().1;
        assert!(c > 0.9, "{c}");
    }

    #[test]
    fn pipeline_run_is_called_pipelines() {
        let mut text = String::from(HEAD);
        for _ in 0..10 {
            text.push('\n');
            text.push_str(&row(400.0, 2000.0, 50.0, 9000.0, 800.0, 100.0).replace(",0,800,0,0,0,0,", ",900,800,40,120,300,200,"));
        }
        let t = Table::parse(&text).unwrap();
        let s = summarize(&t, &world_rows(&t, 500.0));
        let v = verdict(&s);
        assert!(v[0].starts_with("Pipelines"), "{v:?}");
    }

    #[test]
    fn stats() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert!(median(&[]).is_nan());
        assert!((correlation(&[(1.0, 2.0), (2.0, 4.0), (3.0, 6.0)]) - 1.0).abs() < 1e-12);
        assert!((correlation(&[(1.0, 6.0), (2.0, 4.0), (3.0, 2.0)]) + 1.0).abs() < 1e-12);
        assert!(correlation(&[(1.0, 1.0), (1.0, 2.0), (1.0, 3.0)]).is_nan());
    }

    #[test]
    fn short_rows_are_skipped_and_bad_files_refused() {
        let t = Table::parse(&format!("{HEAD}\n1,2,3")).unwrap();
        assert!(t.rows.is_empty());
        assert!(Table::parse("a,b\n1,2").is_err());
        assert!(Table::parse("").is_err());
    }
}
