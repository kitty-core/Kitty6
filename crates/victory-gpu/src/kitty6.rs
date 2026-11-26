//! Kitty6 renderer mods: rules, in files of our own, that change what the
//! title's draws and dispatches do on the host, matched by the hash the
//! runtime names a translated shader by (the first 8 bytes of its BLAKE3, as
//! the log and `victory replay --timing detailed` print them). No part of the
//! title is in a rule file or here: a hash names a shader the user's own copy
//! holds, and the rule says what to do with it.
//!
//! `VICTORY_MODS=<file>[;<file>..]` (`scripts\play.ps1` passes `mods\*.k6mod`
//! of this repository unless `-NoMods`). One rule a line, `#` starts a note:
//!
//! ```text
//! <16 hex digits> skip              # never run
//! <16 hex digits> every <n>         # run in one frame of n (the result stays
//!                                   # in its targets the frames between)
//! <16 hex digits> every <n> at <k>  # ... in frames whose number % n == k
//! ```
//!
//! A rule holds a draw or dispatch back when a pipeline has a shader of that
//! hash (any stage). Held work is not counted as dropped. The dev menu's
//! `mods` item (and `VICTORY_DEV=<t>:mods`) turns all rules off and on while
//! the title runs, to see and time what they change; `VICTORY_MODS_OFF=1`
//! starts with them off.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

/// What a rule does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Skip,
    /// Run when the frame's number % `n` == `at`.
    Every { n: u64, at: u64 },
}

/// A rule of a mod file.
#[derive(Debug)]
pub(crate) struct Rule {
    pub(crate) hash: String,
    pub(crate) action: Action,
    /// The file and line it came from.
    pub(crate) from: String,
    /// Draws and dispatches it held back, and let run.
    held: AtomicU64,
    ran: AtomicU64,
}

/// Rules are applied (the dev menu turns this off and on).
static ON: AtomicBool = AtomicBool::new(true);

/// Every rule of every file.
pub(crate) fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        if std::env::var_os("VICTORY_MODS_OFF").is_some_and(|v| v == "1") {
            ON.store(false, Ordering::Relaxed);
        }
        let files = std::env::var("VICTORY_MODS").unwrap_or_default();
        let mut rules = Vec::new();
        for file in files.split(';').map(str::trim).filter(|f| !f.is_empty()) {
            match std::fs::read_to_string(file) {
                Ok(text) => {
                    let name = std::path::Path::new(file).file_name().map_or(file.into(), |n| n.to_string_lossy().into_owned());
                    let before = rules.len();
                    rules.extend(parse(&text, &name));
                    tracing::warn!(file, rules = rules.len() - before, "Kitty6 mods loaded");
                }
                Err(e) => tracing::warn!(file, error = %e, "Kitty6 mod file not read"),
            }
        }
        rules
    })
}

/// The rules of one file's text; lines that are not rules are said and left out.
pub(crate) fn parse(text: &str, name: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let hash = words[0].to_ascii_lowercase();
        let action = match words[1..] {
            ["skip"] => Some(Action::Skip),
            ["every", n] => n.parse().ok().filter(|&n| n > 0).map(|n| Action::Every { n, at: 0 }),
            ["every", n, "at", k] => match (n.parse::<u64>(), k.parse::<u64>()) {
                (Ok(n), Ok(at)) if n > 0 && at < n => Some(Action::Every { n, at }),
                _ => None,
            },
            _ => None,
        };
        match action {
            Some(action) if hash.len() == 16 && hash.bytes().all(|c| c.is_ascii_hexdigit()) => out.push(Rule {
                hash,
                action,
                from: format!("{name}:{}", i + 1),
                held: AtomicU64::new(0),
                ran: AtomicU64::new(0),
            }),
            _ => tracing::warn!(line, at = %format!("{name}:{}", i + 1), "not a Kitty6 mod rule"),
        }
    }
    out
}

/// The rule for a pipeline of shaders with these hashes, if one names any.
pub(crate) fn rule_for<'a>(hashes: impl IntoIterator<Item = &'a str>) -> Option<u16> {
    let rules = rules();
    if rules.is_empty() {
        return None;
    }
    let hashes: Vec<&str> = hashes.into_iter().collect();
    let i = rules.iter().position(|r| hashes.contains(&r.hash.as_str()))?;
    tracing::info!(rule = %rules[i].from, hash = %rules[i].hash, "a pipeline a Kitty6 mod applies to");
    u16::try_from(i).ok()
}

/// Whether rule `i` holds this frame's draw or dispatch back.
pub(crate) fn holds(i: u16) -> bool {
    let Some(rule) = rules().get(usize::from(i)) else {
        return false;
    };
    let hold = ON.load(Ordering::Relaxed)
        && match rule.action {
            Action::Skip => true,
            Action::Every { n, at } => crate::queue::presented() % n != at,
        };
    if hold { &rule.held } else { &rule.ran }.fetch_add(1, Ordering::Relaxed);
    hold
}

/// Turns the rules off or on; whether they are on now.
pub fn toggle() -> bool {
    let on = !ON.fetch_xor(true, Ordering::Relaxed);
    tracing::warn!(on, rules = rules().len(), "Kitty6 mods (dev menu)");
    on
}

/// Whether the rules are on.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// A line for the log: what each rule held back and let run so far.
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) fn report() -> String {
    rules()
        .iter()
        .map(|r| format!("{} {:?}: {} held, {} ran", r.from, r.action, r.held.load(Ordering::Relaxed), r.ran.load(Ordering::Relaxed)))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_read_and_bad_lines_left_out() {
        let text = "# a note\n\
                    0123456789ABCDEF skip   # loud\n\
                    fedcba9876543210 every 2\n\
                    00000000000000aa every 3 at 1\n\
                    00000000000000ab every 3 at 3\n\
                    short skip\n\
                    00000000000000ac sometimes\n";
        let rules = parse(text, "t.k6mod");
        let got: Vec<(&str, Action)> = rules.iter().map(|r| (r.hash.as_str(), r.action)).collect();
        assert_eq!(got, vec![
            ("0123456789abcdef", Action::Skip),
            ("fedcba9876543210", Action::Every { n: 2, at: 0 }),
            ("00000000000000aa", Action::Every { n: 3, at: 1 }),
        ]);
        assert_eq!(rules[0].from, "t.k6mod:2");
    }
}
