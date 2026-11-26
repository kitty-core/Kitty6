//! `kitty6-prof diff a.json b.json [--top N]`: two runs (two builds, or a
//! switch off and on) side by side, per role: busy and waiting time by kind,
//! then the functions and wait sites whose share changed most. Times are
//! thread-ms per second of wall time (samples / rounds * 1000), shares are
//! % of the role's samples, so runs of different lengths or rates compare.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::analysis::{Kind, is_runtime};
use crate::json::{self, Value};

/// One run's numbers for one role (or all threads).
#[derive(Default)]
struct Side {
    rounds: f64,
    samples: f64,
    threads: f64,
    busy: f64,
    wait: HashMap<String, f64>,
    inclusive: HashMap<String, f64>,
    self_busy: HashMap<String, f64>,
    waits: HashMap<String, f64>,
}

impl Side {
    fn of(v: &Value, rounds: f64) -> Side {
        let map = |key: &str| v.obj(key).iter().map(|(k, n)| (k.clone(), n.num())).collect::<HashMap<_, _>>();
        Side {
            rounds,
            samples: v.f64("samples"),
            threads: v.f64("threads"),
            busy: v.f64("busy"),
            wait: map("wait"),
            inclusive: map("inclusive"),
            self_busy: map("self_busy"),
            waits: v.arr("waits").iter().map(|w| (format!("{:<6} {}", w.str("kind"), w.str("site")), w.f64("samples"))).collect(),
        }
    }

    fn ms(&self, n: f64) -> f64 {
        if self.rounds > 0.0 { 1000.0 * n / self.rounds } else { 0.0 }
    }

    fn pct(&self, n: f64) -> f64 {
        if self.samples > 0.0 { 100.0 * n / self.samples } else { 0.0 }
    }
}

fn load(path: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let v = json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    if !v.str("format").starts_with("kitty6-prof/") {
        return Err(format!("{path}: not a kitty6-prof --json file"));
    }
    Ok(v)
}

/// Rows of the keys whose ms/s changed most: a ms/s, b ms/s, change, change
/// in percentage points of the role's samples.
fn table(r: &mut String, title: &str, a: &Side, b: &Side, pick: impl Fn(&Side) -> &HashMap<String, f64>, top: usize, skip: impl Fn(&str) -> bool) {
    let keys: HashSet<&String> = pick(a).keys().chain(pick(b).keys()).filter(|k| !skip(k)).collect();
    let mut rows: Vec<(f64, f64, f64, f64, &String)> = keys
        .into_iter()
        .map(|k| {
            let (na, nb) = (pick(a).get(k).copied().unwrap_or(0.0), pick(b).get(k).copied().unwrap_or(0.0));
            let (ma, mb) = (a.ms(na), b.ms(nb));
            (ma, mb, mb - ma, b.pct(nb) - a.pct(na), k)
        })
        .filter(|row| row.2.abs() >= 0.05 || row.3.abs() >= 0.05)
        .collect();
    if rows.is_empty() {
        return;
    }
    rows.sort_by(|x, y| y.2.abs().total_cmp(&x.2.abs()).then(y.3.abs().total_cmp(&x.3.abs())).then(x.4.cmp(y.4)));
    rows.truncate(top);
    let _ = writeln!(r, "  -- {title}");
    let _ = writeln!(r, "    {:>9} {:>9} {:>9} {:>8}  name", "a ms/s", "b ms/s", "change", "pp");
    for (ma, mb, d, pp, k) in rows {
        let _ = writeln!(r, "    {ma:>9.1} {mb:>9.1} {d:>+9.1} {pp:>+8.1}  {k}");
    }
}

fn section(r: &mut String, name: &str, a: &Side, b: &Side, top: usize) {
    let only = match (a.samples > 0.0, b.samples > 0.0) {
        (true, false) => " (only in a)",
        (false, true) => " (only in b)",
        _ => "",
    };
    let _ = writeln!(r, "\n== {name}  x{} -> x{}{only}", a.threads, b.threads);
    let _ = writeln!(
        r,
        "  busy      {:>8.1} -> {:>8.1} ms/s ({:>+8.1})   {:>5.1}% -> {:>5.1}% ({:>+5.1} pp)",
        a.ms(a.busy),
        b.ms(b.busy),
        b.ms(b.busy) - a.ms(a.busy),
        a.pct(a.busy),
        b.pct(b.busy),
        b.pct(b.busy) - a.pct(a.busy)
    );
    for k in Kind::ALL {
        let (na, nb) = (a.wait.get(k.name()).copied().unwrap_or(0.0), b.wait.get(k.name()).copied().unwrap_or(0.0));
        if na == 0.0 && nb == 0.0 {
            continue;
        }
        let _ = writeln!(
            r,
            "  {:<9} {:>8.1} -> {:>8.1} ms/s ({:>+8.1})   {:>5.1}% -> {:>5.1}% ({:>+5.1} pp)",
            k.name(),
            a.ms(na),
            b.ms(nb),
            b.ms(nb) - a.ms(na),
            a.pct(na),
            b.pct(nb),
            b.pct(nb) - a.pct(na)
        );
    }
    table(r, "wait sites (kind, site)", a, b, |s| &s.waits, top, |_| false);
    table(r, "self (busy samples)", a, b, |s| &s.self_busy, top, |_| false);
    table(r, "inclusive", a, b, |s| &s.inclusive, top, is_runtime);
}

pub fn diff(a: &Value, b: &Value, top: usize) -> String {
    let (ra, rb) = (a.f64("rounds"), b.f64("rounds"));
    let mut r = String::new();
    let head = |v: &Value| format!("{} pid {}, {} s at {} Hz ({} rounds)", v.str("exe"), v.f64("pid"), v.f64("seconds"), v.f64("hz"), v.f64("rounds"));
    let _ = writeln!(r, "kitty6-prof diff (ms/s = thread-ms per second of wall time; pp = points of the role's samples)");
    let _ = writeln!(r, "  a: {}\n  b: {}", head(a), head(b));
    let empty = Value::Obj(Vec::new());
    section(&mut r, "all threads", &Side::of(a.get("all").unwrap_or(&empty), ra), &Side::of(b.get("all").unwrap_or(&empty), rb), top);
    // Roles by their larger sample count over the two runs.
    let mut roles: Vec<(&str, f64)> = Vec::new();
    for v in a.arr("roles").iter().chain(b.arr("roles")) {
        let (name, n) = (v.str("role"), v.f64("samples"));
        match roles.iter_mut().find(|(r, _)| *r == name) {
            Some(x) => x.1 = x.1.max(n),
            None => roles.push((name, n)),
        }
    }
    roles.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(y.0)));
    let find = |v: &Value, role: &str, rounds: f64| {
        v.arr("roles").iter().find(|x| x.str("role") == role).map_or_else(|| Side { rounds, ..Default::default() }, |x| Side::of(x, rounds))
    };
    for (role, _) in roles {
        section(&mut r, &format!("role {role}"), &find(a, role, ra), &find(b, role, rb), top);
    }
    r
}

/// `diff a.json b.json [--top N] [--out FILE]`.
pub fn run(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let (mut top, mut out) = (10usize, None);
    let mut it = args.iter();
    while let Some(k) = it.next() {
        match k.as_str() {
            "--top" => top = it.next().and_then(|v| v.parse().ok()).unwrap_or(10),
            "--out" => out = it.next().cloned(),
            _ => files.push(k.clone()),
        }
    }
    if files.len() != 2 {
        eprintln!("usage: kitty6-prof diff a.json b.json [--top N] [--out FILE]");
        return 2;
    }
    let (a, b) = match (load(&files[0]), load(&files[1])) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let r = diff(&a, &b, top);
    print!("{r}");
    if let Some(out) = out {
        let _ = std::fs::write(&out, &r);
        eprintln!("written to {out}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{Options, analyze, tests::profile, to_json};

    #[test]
    fn a_lock_that_goes_away() {
        let lock = vec!["ntdll!NtWaitForAlertByThreadId", "ntdll!RtlAcquireSRWLockExclusive", "victory_gpu::shadow::exact", "victory_gpu::list::record"];
        let run = vec!["victory_gpu::list::record_draw", "victory_gpu::list::record"];
        let a = profile(vec![(1, "victory-list-#", vec![(lock.clone(), 75), (run.clone(), 25)])]);
        let b = profile(vec![(1, "victory-list-#", vec![(lock, 10), (run, 90)])]);
        let ja = json::parse(&to_json(&a, &analyze(&a, &Options::default()))).unwrap();
        let jb = json::parse(&to_json(&b, &analyze(&b, &Options::default()))).unwrap();
        let d = diff(&ja, &jb, 5);
        assert!(d.contains("== role list recorders  x1 -> x1"), "{d}");
        assert!(d.contains("  busy         250.0 ->    900.0 ms/s (  +650.0)    25.0% ->  90.0% (+65.0 pp)"), "{d}");
        assert!(d.contains("  lock         750.0 ->    100.0 ms/s (  -650.0)"), "{d}");
        assert!(d.contains("      750.0     100.0    -650.0    -65.0  lock   victory_gpu::shadow::exact"), "{d}");
        assert!(d.contains("      250.0     900.0    +650.0    +65.0  victory_gpu::list::record_draw"), "{d}");
    }

    #[test]
    fn roles_in_one_run_only() {
        let a = profile(vec![(1, "victory-submit", vec![(vec!["victory_gpu::host::Host::submit"], 10)])]);
        let b = profile(vec![(2, "victory-relay", vec![(vec!["victory_gpu::host::relay_worker"], 10)])]);
        let ja = json::parse(&to_json(&a, &analyze(&a, &Options::default()))).unwrap();
        let jb = json::parse(&to_json(&b, &analyze(&b, &Options::default()))).unwrap();
        let d = diff(&ja, &jb, 5);
        assert!(d.contains("== role submission  x1 -> x0 (only in a)"), "{d}");
        assert!(d.contains("== role victory-relay  x0 -> x1 (only in b)"), "{d}");
    }
}
