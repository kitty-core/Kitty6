//! Diagnostics: the dev menu, a trainer-like list of switches drawn in the
//! panel's box ([`crate::host`]'s panel) while the title runs.
//!
//! F4 opens and closes it (one of our windows in front); Page Up and Page
//! Down pick an item, Insert carries it out. While it is open those keys are
//! kept from the title, and F4 always is.
//!
//! The same items can be run at set times without hands, for scripted runs:
//! `VICTORY_DEV=<seconds>:<item>[,<seconds>:<item>..]`, seconds after the
//! first presented frame, an item being one of `panel`, `footage`, `rate`,
//! `shot`, `timeline`, `snapshot`, `autopilot`, `mark`.
//!
//! - **panel**: the run's numbers on the picture on or off (as F3).
//! - **footage**: presented frames saved as PNG files, half size, at the rate
//!   below, into `%LOCALAPPDATA%\victory\footage\<start time>\`
//!   (`VICTORY_FOOTAGE_DIR` for another folder): gameplay footage to look at
//!   afterwards. Each file is named by the seconds since the first frame and
//!   the frame's number. Encoding runs on a thread of its own; the frame read
//!   back waits for its own list (a few ms a saved frame).
//! - **rate**: 1, 2 or 5 saved frames a second.
//! - **shot**: one frame saved now, into the same folder.
//! - **timeline**: a 3-frame timeline now, when the run was started with
//!   `VICTORY_TIMELINE=key,..` (`scripts\play.ps1 -F8Timeline`), as F8.
//! - **snapshot**: the panel's numbers written to the log (warn), with the
//!   seconds since the first frame: to line them up with footage.
//! - **autopilot**: keys held for the title: off, walk (W), run (W and Shift),
//!   circle (W, with A held one second in four).
//! - **mark**: a numbered line in the log, and a marked file in the footage.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The items, in the menu's order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Item {
    Panel,
    Footage,
    Rate,
    Shot,
    Timeline,
    Snapshot,
    Autopilot,
    Mark,
}

const ITEMS: [Item; 8] =
    [Item::Panel, Item::Footage, Item::Rate, Item::Shot, Item::Timeline, Item::Snapshot, Item::Autopilot, Item::Mark];

impl Item {
    fn named(name: &str) -> Option<Item> {
        ITEMS.iter().copied().find(|i| i.name() == name.trim().to_ascii_lowercase())
    }

    fn name(self) -> &'static str {
        match self {
            Item::Panel => "panel",
            Item::Footage => "footage",
            Item::Rate => "rate",
            Item::Shot => "shot",
            Item::Timeline => "timeline",
            Item::Snapshot => "snapshot",
            Item::Autopilot => "autopilot",
            Item::Mark => "mark",
        }
    }
}

/// Saved frames a second, by [`RATE`].
const RATES: [f64; 3] = [1.0, 2.0, 5.0];
/// What the autopilot holds, by [`AUTOPILOT`].
const PILOTS: [&str; 4] = ["OFF", "WALK", "RUN", "CIRCLE"];

static OPEN: AtomicBool = AtomicBool::new(false);
static AT: AtomicUsize = AtomicUsize::new(0);
static FOOTAGE: AtomicBool = AtomicBool::new(false);
static RATE: AtomicUsize = AtomicUsize::new(1);
static AUTOPILOT: AtomicU8 = AtomicU8::new(0);
/// Asked for and not yet taken.
static SHOT: AtomicBool = AtomicBool::new(false);
static TIMELINE: AtomicBool = AtomicBool::new(false);
static SNAPSHOT: AtomicBool = AtomicBool::new(false);
static PANEL: AtomicBool = AtomicBool::new(false);
static MARKS: AtomicU64 = AtomicU64::new(0);
/// The last footage frame's time, in ms since the first frame.
static LAST_SAVED_MS: AtomicU64 = AtomicU64::new(0);
/// The menu or what it shows changed since the panel last looked.
static CHANGED: AtomicBool = AtomicBool::new(false);

/// The first presented frame, which times count from.
fn started() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Seconds since the first presented frame.
pub(crate) fn seconds() -> f64 {
    started().elapsed().as_secs_f64()
}

/// Whether the menu is open.
pub fn open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Whether the title should not see this key (virtual-key code): F4 always,
/// the menu's keys while it is open.
pub fn kept_from_title(vk: u8) -> bool {
    const F4: u8 = 0x73;
    vk == F4 || (open() && matches!(vk, 0x21 | 0x22 | 0x2D))
}

/// Keys the autopilot holds now (virtual-key codes).
pub fn held_keys() -> Vec<u8> {
    const W: u8 = b'W';
    const A: u8 = b'A';
    const SHIFT: u8 = 0xA0;
    match AUTOPILOT.load(Ordering::Relaxed) {
        1 => vec![W],
        2 => vec![W, SHIFT],
        3 if (seconds() as u64) % 4 == 0 => vec![W, A],
        3 => vec![W],
        _ => Vec::new(),
    }
}

/// The key went down since the last call, one of our windows in front.
fn pressed(vk: u16) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    static DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
    // SAFETY: a plain query.
    let down = unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
    if DOWN[usize::from(vk & 0xFF)].swap(down, Ordering::Relaxed) || !down {
        return false;
    }
    let mut process = 0;
    // SAFETY: plain queries; room for the answer.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process == std::process::id()
}

/// `VICTORY_DEV`'s items by time, and how many have run.
fn script() -> &'static Mutex<(Vec<(f64, Item)>, usize)> {
    static SCRIPT: OnceLock<Mutex<(Vec<(f64, Item)>, usize)>> = OnceLock::new();
    SCRIPT.get_or_init(|| {
        let text = std::env::var("VICTORY_DEV").unwrap_or_default();
        let mut items: Vec<(f64, Item)> = text
            .split(',')
            .filter_map(|entry| {
                let (at, name) = entry.split_once(':')?;
                let item = Item::named(name);
                if item.is_none() {
                    tracing::warn!(entry, "VICTORY_DEV: no such item");
                }
                Some((at.trim().parse().ok()?, item?))
            })
            .collect();
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        Mutex::new((items, 0))
    })
}

/// Looks at the keys and the script; called at every present.
pub(crate) fn poll() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_F4, VK_INSERT, VK_NEXT, VK_PRIOR};
    let now = seconds();
    {
        let mut script = script().lock().unwrap_or_else(|e| e.into_inner());
        while let Some(&(at, item)) = script.0.get(script.1) {
            if at > now {
                break;
            }
            script.1 += 1;
            tracing::warn!(item = item.name(), at, "dev menu item run (VICTORY_DEV)");
            run(item);
        }
    }
    if pressed(VK_F4) {
        OPEN.fetch_xor(true, Ordering::Relaxed);
        CHANGED.store(true, Ordering::Relaxed);
    }
    if !open() {
        return;
    }
    let n = ITEMS.len();
    if pressed(VK_PRIOR) {
        AT.store((AT.load(Ordering::Relaxed) + n - 1) % n, Ordering::Relaxed);
        CHANGED.store(true, Ordering::Relaxed);
    }
    if pressed(VK_NEXT) {
        AT.store((AT.load(Ordering::Relaxed) + 1) % n, Ordering::Relaxed);
        CHANGED.store(true, Ordering::Relaxed);
    }
    if pressed(VK_INSERT) {
        run(ITEMS[AT.load(Ordering::Relaxed) % n]);
    }
}

/// Carries an item out.
fn run(item: Item) {
    match item {
        Item::Panel => PANEL.store(true, Ordering::Relaxed),
        Item::Footage => {
            let on = !FOOTAGE.fetch_xor(true, Ordering::Relaxed);
            tracing::warn!(on, folder = ?footage_folder().map(|f| f.display().to_string()), "footage (dev menu)");
        }
        Item::Rate => {
            RATE.store((RATE.load(Ordering::Relaxed) + 1) % RATES.len(), Ordering::Relaxed);
        }
        Item::Shot => SHOT.store(true, Ordering::Relaxed),
        Item::Timeline => TIMELINE.store(true, Ordering::Relaxed),
        Item::Snapshot => SNAPSHOT.store(true, Ordering::Relaxed),
        Item::Autopilot => {
            let next = (AUTOPILOT.load(Ordering::Relaxed) + 1) % PILOTS.len() as u8;
            AUTOPILOT.store(next, Ordering::Relaxed);
            tracing::warn!(autopilot = PILOTS[usize::from(next)], at = seconds(), "autopilot (dev menu)");
        }
        Item::Mark => {
            let n = MARKS.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(mark = n, at = seconds(), "mark (dev menu)");
            SHOT.store(true, Ordering::Relaxed);
        }
    }
    CHANGED.store(true, Ordering::Relaxed);
}

/// The panel's numbers were asked to show or hide.
pub(crate) fn take_panel_toggle() -> bool {
    PANEL.swap(false, Ordering::Relaxed)
}

/// A timeline was asked for (as F8).
pub(crate) fn take_timeline() -> bool {
    TIMELINE.swap(false, Ordering::Relaxed)
}

/// The panel's numbers are to go to the log.
pub(crate) fn take_snapshot() -> bool {
    SNAPSHOT.swap(false, Ordering::Relaxed)
}

/// What the menu shows changed since the last call.
pub(crate) fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::Relaxed)
}

/// Whether presented frame `frame` is to be saved (footage or a shot).
/// Called once a frame.
pub(crate) fn frame_wanted(_frame: u64) -> bool {
    if SHOT.swap(false, Ordering::Relaxed) {
        return true;
    }
    if !FOOTAGE.load(Ordering::Relaxed) {
        return false;
    }
    let now = (seconds() * 1e3) as u64;
    let every = (1e3 / RATES[RATE.load(Ordering::Relaxed) % RATES.len()]) as u64;
    let last = LAST_SAVED_MS.load(Ordering::Relaxed);
    if now >= last + every || last == 0 {
        LAST_SAVED_MS.store(now, Ordering::Relaxed);
        return true;
    }
    false
}

/// The menu's lines (capitals, digits and `./-:>` only: the panel's letters).
pub(crate) fn lines() -> Vec<String> {
    let at = AT.load(Ordering::Relaxed) % ITEMS.len();
    let on = |b: bool| if b { "ON" } else { "OFF" };
    let timeline = if crate::timeline::keyed() { "NOW" } else { "NEEDS -F8TIMELINE" };
    let mut out = vec!["DEV MENU  F4 CLOSE  PGUP/PGDN  INS".to_owned()];
    for (i, item) in ITEMS.iter().enumerate() {
        let value = match item {
            Item::Panel => "NUMBERS".to_owned(),
            Item::Footage => on(FOOTAGE.load(Ordering::Relaxed)).to_owned(),
            Item::Rate => format!("{} A SECOND", RATES[RATE.load(Ordering::Relaxed) % RATES.len()]),
            Item::Shot => "SAVE ONE FRAME".to_owned(),
            Item::Timeline => timeline.to_owned(),
            Item::Snapshot => "NUMBERS TO LOG".to_owned(),
            Item::Autopilot => PILOTS[usize::from(AUTOPILOT.load(Ordering::Relaxed)) % PILOTS.len()].to_owned(),
            Item::Mark => format!("{} SO FAR", MARKS.load(Ordering::Relaxed)),
        };
        let cursor = if i == at { '>' } else { ' ' };
        out.push(format!("{cursor}{:<10}{value}", item.name().to_ascii_uppercase()));
    }
    out.push(format!("T {:.0} S", seconds()));
    out
}

/// The folder footage goes to, made at first use.
pub(crate) fn footage_folder() -> Option<&'static PathBuf> {
    static FOLDER: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let folder = match std::env::var_os("VICTORY_FOOTAGE_DIR").filter(|v| !v.is_empty()) {
                Some(dir) => PathBuf::from(dir),
                None => {
                    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("victory").join("footage");
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                    base.join(stamp.to_string())
                }
            };
            std::fs::create_dir_all(&folder).ok()?;
            Some(folder)
        })
        .as_ref()
}

/// A frame read back for footage: its rows, copied out of the mapping, are
/// encoded on a thread of its own.
pub(crate) fn save(frame: u64, buffer: &windows::Win32::Graphics::Direct3D12::ID3D12Resource, footprint: &windows::Win32::Graphics::Direct3D12::D3D12_PLACED_SUBRESOURCE_FOOTPRINT) {
    use std::sync::mpsc::{Sender, channel};
    let Some(folder) = footage_folder() else { return };
    let f = footprint.Footprint;
    let len = f.RowPitch as usize * f.Height as usize;
    let mut data = std::ptr::null_mut();
    // SAFETY: the copy finished (the frame's list was waited for); the
    // mapping spans the footprint's rows.
    let rows = unsafe {
        if buffer.Map(0, None, Some(&mut data)).is_err() {
            return;
        }
        let rows = std::slice::from_raw_parts(data.cast::<u8>(), len).to_vec();
        buffer.Unmap(0, None);
        rows
    };
    type Job = (PathBuf, u32, u32, usize, i32, Vec<u8>);
    static WORKER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (send, take) = channel::<Job>();
        let _ = std::thread::Builder::new().name("victory-footage".into()).spawn(move || {
            for (path, w, h, pitch, format, rows) in take {
                if let Some(png) = to_png(w, h, pitch, format, &rows) {
                    let _ = std::fs::write(&path, png);
                }
            }
        });
        Mutex::new(send)
    });
    let marks = MARKS.load(Ordering::Relaxed);
    let name = format!("t{:07.1}-f{frame}{}.png", seconds(), if marks > 0 { format!("-m{marks}") } else { String::new() });
    let job = (folder.join(name), f.Width, f.Height, f.RowPitch as usize, f.Format.0, rows);
    let _ = worker.lock().unwrap_or_else(|e| e.into_inner()).send(job);
}

/// Half-size 8-bit RGB PNG of rows in one of the swap chain's formats.
fn to_png(width: u32, height: u32, pitch: usize, format: i32, rows: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Dxgi::Common::*;
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let pixel: fn(&[u8]) -> [u8; 3] = match DXGI_FORMAT(format) {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => |p| [p[0], p[1], p[2]],
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => |p| [p[2], p[1], p[0]],
        DXGI_FORMAT_R10G10B10A2_UNORM => |p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            [(v >> 2 & 0xFF) as u8, (v >> 12 & 0xFF) as u8, (v >> 22 & 0xFF) as u8]
        },
        DXGI_FORMAT_R16G16B16A16_FLOAT => |p| {
            let c = |i: usize| {
                let f = half(u16::from_le_bytes([p[i], p[i + 1]])).clamp(0.0, 1.0);
                // (linear to sRGB, roughly)
                (f.powf(1.0 / 2.2) * 255.0 + 0.5) as u8
            };
            [c(0), c(2), c(4)]
        },
        _ => return None,
    };
    let bytes = match DXGI_FORMAT(format) {
        DXGI_FORMAT_R16G16B16A16_FLOAT => 8,
        _ => 4,
    };
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        let row = rows.get((2 * y).min(height as usize - 1) * pitch..)?;
        for x in 0..w as usize {
            let at = (2 * x).min(width as usize - 1) * bytes;
            rgb.extend(pixel(row.get(at..at + bytes)?));
        }
    }
    Some(crate::capture::png(w, h, &rgb))
}

fn half(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1F);
    let man = f32::from(h & 0x3FF);
    sign * match exp {
        0 => man / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_named_both_ways() {
        for item in ITEMS {
            assert_eq!(Item::named(item.name()), Some(item));
            assert_eq!(Item::named(&item.name().to_ascii_uppercase()), Some(item));
        }
        assert_eq!(Item::named("nothing"), None);
    }

    #[test]
    fn the_menu_uses_only_the_panels_letters() {
        for line in lines() {
            assert!(line.len() <= 40, "{line}");
            assert!(line.bytes().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase() || b" ./-:>".contains(&c)), "{line}");
        }
    }

    #[test]
    fn ten_bit_and_half_pixels_become_rgb() {
        // 2x2 of R10G10B10A2 white: half size is one white pixel.
        let white = (0x3FFu32 | 0x3FF << 10 | 0x3FF << 20 | 3 << 30).to_le_bytes();
        let rows: Vec<u8> = white.iter().copied().cycle().take(16).collect();
        let png = to_png(2, 2, 8, windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R10G10B10A2_UNORM.0, &rows).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0), 0.0);
    }
}
