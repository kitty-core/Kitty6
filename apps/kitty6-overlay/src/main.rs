//! `kitty6-overlay`: a live profiler HUD drawn over the game's window.
//!
//! It reads the telemetry block a diagnostics build of the game publishes
//! (`victory_core::telemetry`, written by `victory_gpu`'s `telemetry`
//! module) and draws two click-through, topmost strips down the left and
//! right edges of the game's client area. F6 hides and shows it; hidden, it
//! tells the game to stop sampling, so the game pays for the numbers only
//! while they are on screen. Ctrl+Alt+F6 quits.
//!
//! ```text
//! kitty6-overlay                     follow the game (scripts\play.ps1 -Overlay starts it)
//! kitty6-overlay --exit-with-game    the same, and quit once the game has gone
//! kitty6-overlay --demo              a stand-in window with made-up numbers
//! kitty6-overlay --demo --shot <png> draw the demo off screen once, save it, quit
//! ```

#[cfg(windows)]
mod demo;
#[cfg(windows)]
mod link;
mod model;
#[cfg(windows)]
mod paint;
#[cfg(windows)]
mod panels;
mod png;

#[cfg(not(windows))]
fn main() {
    eprintln!("kitty6-overlay runs on Windows only");
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    if flag("--help") || flag("-h") {
        println!(
            "kitty6-overlay [--demo] [--shot <png>] [--exit-with-game]\n  F6 shows/hides (and turns the game's sampling on/off), Ctrl+Alt+F6 quits."
        );
        return;
    }
    let shot = args
        .iter()
        .position(|a| a == "--shot")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let result = match shot {
        Some(path) => app::shot(std::path::Path::new(&path)),
        None => app::run(flag("--demo"), flag("--exit-with-game")),
    };
    if let Err(e) = result {
        eprintln!("kitty6-overlay: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod app {
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::{Duration, Instant};

    use victory_core::telemetry::{DEMO_NAME, FrameRing, NAME, Stats};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::{ClientToScreen, CreateSolidBrush, HBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
        VK_F6,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{BOOL, Result, w};

    use crate::link::{Link, now_ms};
    use crate::paint::{Kit, Surface};
    use crate::panels::{self, HEADER_HEIGHT, STRIP_WIDTH, View};
    use crate::{demo, model, png};

    const HOTKEY_TOGGLE: i32 = 1;
    const HOTKEY_QUIT: i32 = 2;
    const TICK: Duration = Duration::from_millis(33);

    /// The content heights of the strips at scale 1.
    const LEFT_HEIGHT: f32 = 820.0;
    const RIGHT_HEIGHT: f32 = 920.0;

    unsafe extern "system" fn overlay_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            // SAFETY: the window's own message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    unsafe extern "system" fn demo_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_DESTROY {
            // SAFETY: plain call.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        // SAFETY: the window's own message.
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    fn overlay_window(instance: HINSTANCE) -> Result<HWND> {
        // SAFETY: a popup window of our own class.
        unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOPMOST,
                w!("Kitty6Overlay"),
                w!("Kitty6 Overlay"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
        }
    }

    fn register_classes(instance: HINSTANCE) {
        // SAFETY: classes of ours with static names and procedures.
        unsafe {
            let overlay = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: w!("Kitty6Overlay"),
                ..Default::default()
            };
            RegisterClassExW(&overlay);
            let stand_in = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(demo_proc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH(CreateSolidBrush(COLORREF(0x0024_1810)).0),
                lpszClassName: w!("Kitty6DemoGame"),
                ..Default::default()
            };
            RegisterClassExW(&stand_in);
        }
    }

    /// The game's client area on the screen: x, y, width, height.
    fn client_area(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
        // SAFETY: queries of a window that may have gone (they then fail).
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool()
                || IsIconic(hwnd).as_bool()
                || !IsWindowVisible(hwnd).as_bool()
            {
                return None;
            }
            let mut r = RECT::default();
            GetClientRect(hwnd, &mut r).ok()?;
            let mut at = POINT { x: 0, y: 0 };
            if !ClientToScreen(hwnd, &mut at).as_bool() {
                return None;
            }
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            (w > 64 && h > 64).then_some((at.x, at.y, w, h))
        }
    }

    fn window_pid(hwnd: HWND) -> u32 {
        let mut pid = 0;
        // SAFETY: plain query.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        pid
    }

    /// The largest visible top-level window of process `pid`.
    fn main_window(pid: u32) -> Option<HWND> {
        struct Search {
            pid: u32,
            best: Option<(HWND, i64)>,
        }
        unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
            // SAFETY: `lp` is the search below, alive during the enumeration.
            let search = unsafe { &mut *(lp.0 as *mut Search) };
            // SAFETY: plain queries.
            let owned = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid());
            if window_pid(hwnd) == search.pid && !owned {
                let mut r = RECT::default();
                // SAFETY: plain queries.
                let visible = unsafe {
                    IsWindowVisible(hwnd).as_bool() && GetClientRect(hwnd, &mut r).is_ok()
                };
                let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
                if visible && search.best.is_none_or(|b| area > b.1) {
                    search.best = Some((hwnd, area));
                }
            }
            TRUE
        }
        let mut search = Search { pid, best: None };
        // SAFETY: the callback reads `search` only during the call.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize)) };
        search.best.map(|b| b.0)
    }

    /// The window to follow: the one the game presents to, else its largest.
    fn game_window(link: &Link, pid: u32) -> Option<HWND> {
        let announced = HWND(link.header().hwnd.load(Relaxed) as usize as *mut _);
        // SAFETY: plain query.
        if !announced.is_invalid()
            && unsafe { IsWindow(Some(announced)).as_bool() }
            && window_pid(announced) == pid
        {
            return Some(announced);
        }
        main_window(pid)
    }

    /// One layered window and its pixels.
    struct Strip {
        hwnd: HWND,
        surface: Option<Surface>,
        shown: bool,
    }

    impl Strip {
        #[allow(clippy::too_many_arguments)]
        fn place(
            &mut self,
            kit: &Kit,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            scale: f32,
            draw: impl FnOnce(&crate::paint::Painter),
        ) {
            if self
                .surface
                .as_ref()
                .is_none_or(|s| s.width != w || s.height != h)
            {
                self.surface = Surface::new(kit, w, h).ok();
            }
            let Some(surface) = &self.surface else { return };
            if surface.draw(kit, scale, draw).is_err()
                || surface.show(self.hwnd, POINT { x, y }).is_err()
            {
                return;
            }
            if !self.shown {
                // SAFETY: our window; shown without taking the focus.
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                }
                self.shown = true;
            }
        }

        fn hide(&mut self) {
            if self.shown {
                // SAFETY: our window.
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                self.shown = false;
            }
        }

        fn keep_on_top(&self) {
            if self.shown {
                // SAFETY: our window.
                unsafe {
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
                    );
                }
            }
        }
    }

    fn scale_for(height: i32) -> f32 {
        (height as f32 / 1080.0).clamp(0.6, 2.0)
    }

    /// The overlay, following the game (or the demo's stand-in).
    pub fn run(demo_mode: bool, exit_with_game: bool) -> Result<()> {
        // SAFETY: process-wide settings and our own windows and hotkeys.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        // SAFETY: plain query.
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        register_classes(instance);
        let kit = Kit::new()?;
        let link = Link::open(if demo_mode { DEMO_NAME } else { NAME })?;

        // The demo: a stand-in game window and a thread publishing into the demo block.
        if demo_mode {
            // SAFETY: a normal window of our class.
            let stand_in = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("Kitty6DemoGame"),
                    w!("Kitty6 demo (stand-in game window)"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    120,
                    80,
                    1600,
                    900,
                    None,
                    None,
                    Some(instance),
                    None,
                )?
            };
            let block = link.block();
            let hwnd = stand_in.0 as usize;
            std::thread::spawn(move || {
                let mut sim = demo::Sim::new();
                demo::announce(block, hwnd);
                demo::fill(block, &mut sim, 90.0);
                loop {
                    let ms = demo::step(block, &mut sim);
                    std::thread::sleep(Duration::from_secs_f32(ms.min(250.0) / 1000.0));
                }
            });
        }

        let mut strips = [overlay_window(instance)?, overlay_window(instance)?].map(|hwnd| Strip {
            hwnd,
            surface: None,
            shown: false,
        });
        // SAFETY: thread hotkeys, unregistered below.
        let hotkey = unsafe {
            RegisterHotKey(None, HOTKEY_TOGGLE, MOD_NOREPEAT, u32::from(VK_F6.0)).is_ok()
        };
        // SAFETY: as above.
        let _ = unsafe {
            RegisterHotKey(
                None,
                HOTKEY_QUIT,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                u32::from(VK_F6.0),
            )
        };
        if !hotkey {
            eprintln!("kitty6-overlay: F6 is taken by another program; watching the key instead");
        }

        let mut visible = true;
        let mut f6_down = false;
        let mut frames = Box::new(FrameRing::ZERO);
        let mut stats = Box::new(Stats::ZERO);
        let mut seen_game = false;
        let mut gone_since: Option<Instant> = None;
        let mut last_top = Instant::now();
        let mut next = Instant::now();
        'run: loop {
            let mut msg = MSG::default();
            // SAFETY: our thread's queue.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() } {
                match msg.message {
                    WM_QUIT => break 'run,
                    WM_HOTKEY if msg.wParam.0 == HOTKEY_TOGGLE as usize => visible = !visible,
                    WM_HOTKEY if msg.wParam.0 == HOTKEY_QUIT as usize => break 'run,
                    _ => {
                        // SAFETY: as above.
                        unsafe {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }
            }
            if !hotkey {
                // SAFETY: plain query.
                let down = unsafe { GetAsyncKeyState(i32::from(VK_F6.0)) } as u16 & 0x8000 != 0;
                if down && !f6_down {
                    visible = !visible;
                }
                f6_down = down;
            }
            let now = Instant::now();
            if now >= next {
                next = now + TICK;
                link.beat(visible);
                let game = link.game();
                seen_game |= game.is_some();
                gone_since = if game.is_some() {
                    None
                } else {
                    gone_since.or(Some(now))
                };
                if exit_with_game
                    && seen_game
                    && gone_since.is_some_and(|t| t.elapsed() > Duration::from_secs(5))
                {
                    break 'run;
                }
                tick(
                    &kit,
                    &link,
                    &mut strips,
                    visible,
                    game,
                    hotkey,
                    demo_mode,
                    &mut frames,
                    &mut stats,
                );
                if last_top.elapsed() > Duration::from_secs(1) {
                    last_top = Instant::now();
                    strips.iter().for_each(Strip::keep_on_top);
                }
            }
            let wait = next.saturating_duration_since(Instant::now()).as_millis() as u32;
            // SAFETY: waits for input or the next tick.
            unsafe { MsgWaitForMultipleObjects(None, false, wait.max(1), QS_ALLINPUT) };
        }
        link.beat(false);
        // SAFETY: ours.
        unsafe {
            let _ = UnregisterHotKey(None, HOTKEY_TOGGLE);
            let _ = UnregisterHotKey(None, HOTKEY_QUIT);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn tick(
        kit: &Kit,
        link: &Link,
        strips: &mut [Strip; 2],
        visible: bool,
        game: Option<u32>,
        hotkey: bool,
        demo_mode: bool,
        frames: &mut FrameRing,
        stats: &mut Stats,
    ) {
        if !visible {
            strips.iter_mut().for_each(Strip::hide);
            return;
        }
        let blink = (now_ms() / 500).is_multiple_of(2);
        let Some(pid) = game else {
            // No game: a small badge in the corner of the main screen.
            let s = 1.0;
            let view = View {
                connected: false,
                syncing: false,
                pid: 0,
                frames,
                stats,
                causes: &[],
                hotkey,
                demo: demo_mode,
                blink,
            };
            let (w, h) = ((STRIP_WIDTH * s) as i32, (HEADER_HEIGHT * s) as i32);
            strips[0].place(kit, 16, 16, w, h, s, |p| panels::left(p, w as f32, &view));
            strips[1].hide();
            return;
        };
        let area = game_window(link, pid).and_then(client_area);
        // In front only while the game is (or one of ours, or nothing, is).
        // SAFETY: plain queries.
        let front = unsafe { GetForegroundWindow() };
        let front_pid = if front.is_invalid() {
            0
        } else {
            window_pid(front)
        };
        let in_front = front_pid == 0 || front_pid == pid || front_pid == std::process::id();
        let Some((x, y, w, h)) = area.filter(|_| in_front) else {
            strips.iter_mut().for_each(Strip::hide);
            return;
        };
        if !link.read(frames, stats) {
            return; // the writer kept getting in the way: the last picture stays
        }
        let syncing = stats.seconds == 0
            || now_ms().saturating_sub(link.header().game_tick_ms.load(Relaxed)) > 3000;
        let causes = model::verdict(stats);
        let view = View {
            connected: true,
            syncing,
            pid,
            frames,
            stats,
            causes: &causes,
            hotkey,
            demo: demo_mode,
            blink,
        };
        let s = scale_for(h);
        let sw = ((STRIP_WIDTH * s) as i32).min(w / 3);
        let lh = ((LEFT_HEIGHT * s) as i32).min(h);
        let rh = ((RIGHT_HEIGHT * s) as i32).min(h);
        strips[0].place(kit, x, y, sw, lh, s, |p| panels::left(p, sw as f32, &view));
        strips[1].place(kit, x + w - sw, y, sw, rh, s, |p| {
            panels::right(p, sw as f32, &view)
        });
    }

    /// The demo drawn once off screen at 1920x1080 over a stand-in picture,
    /// saved as a PNG (no window is shown, no hotkey taken).
    pub fn shot(path: &std::path::Path) -> Result<()> {
        let kit = Kit::new()?;
        let link = Link::open(&format!("{DEMO_NAME}Shot{}", std::process::id()))?;
        let block = link.block();
        let mut sim = demo::Sim::new();
        demo::announce(block, 0);
        // Into the minute's paging storm, so the picture shows the alarms.
        demo::fill(block, &mut sim, 165.0);
        let mut frames = Box::new(FrameRing::ZERO);
        let mut stats = Box::new(Stats::ZERO);
        link.read(&mut frames, &mut stats);
        let causes = model::verdict(&stats);
        let view = View {
            connected: true,
            syncing: false,
            pid: std::process::id(),
            frames: &frames,
            stats: &stats,
            causes: &causes,
            hotkey: true,
            demo: true,
            blink: true,
        };
        let (width, height) = (1920i32, 1080i32);
        let s = scale_for(height);
        let sw = (STRIP_WIDTH * s) as i32;
        let left = Surface::new(&kit, sw, (LEFT_HEIGHT * s) as i32)?;
        left.draw(&kit, s, |p| panels::left(p, sw as f32, &view))?;
        let right = Surface::new(&kit, sw, (RIGHT_HEIGHT * s) as i32)?;
        right.draw(&kit, s, |p| panels::right(p, sw as f32, &view))?;

        let mut rgb = backdrop(width as usize, height as usize);
        for (surface, x0) in [(&left, 0usize), (&right, (width - sw) as usize)] {
            let px = surface.pixels();
            for y in 0..surface.height as usize {
                for x in 0..surface.width as usize {
                    let i = (y * surface.width as usize + x) * 4;
                    let (b, g, r, a) = (px[i], px[i + 1], px[i + 2], px[i + 3]);
                    let o = (y * width as usize + x0 + x) * 3;
                    let keep = 255 - u32::from(a);
                    for (k, src) in [r, g, b].into_iter().enumerate() {
                        let dst = u32::from(rgb[o + k]);
                        rgb[o + k] = (u32::from(src) + dst * keep / 255).min(255) as u8;
                    }
                }
            }
        }
        std::fs::write(path, png::encode(width as u32, height as u32, &rgb))
            .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
        println!("kitty6-overlay: demo picture written to {}", path.display());
        Ok(())
    }

    /// A stand-in for the game's picture: a dusk sky over a skyline.
    fn backdrop(w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 3];
        let mut heights = vec![0usize; w];
        let mut seed = 12345u32;
        let mut x = 0;
        while x < w {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let bw = 30 + (seed >> 16) as usize % 90;
            let bh = h / 5 + (seed >> 8) as usize % (h / 2);
            for slot in heights.iter_mut().skip(x).take(bw) {
                *slot = bh;
            }
            x += bw;
        }
        for y in 0..h {
            let t = y as f32 / h as f32;
            let sky = [
                (40.0 + 150.0 * t * t) as u8,
                (30.0 + 70.0 * t * t) as u8,
                (70.0 + 30.0 * (1.0 - t)) as u8,
            ];
            for x in 0..w {
                let o = (y * w + x) * 3;
                let building = h - y < heights[x];
                let cell = ((x / 9) as u32).wrapping_mul(2_654_435_761)
                    ^ ((y / 14) as u32).wrapping_mul(40_503);
                let lit = building
                    && cell.is_multiple_of(7)
                    && (2..6).contains(&(x % 9))
                    && (4..10).contains(&(y % 14));
                let c = if lit {
                    [200, 170, 90]
                } else if building {
                    [14, 12, 22]
                } else {
                    sky
                };
                out[o..o + 3].copy_from_slice(&c);
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A layered window takes a drawn surface (off screen and never
        /// shown: the test must not put anything over the desktop).
        #[test]
        fn layered_window_takes_the_pixels() {
            // SAFETY: plain query.
            let instance: HINSTANCE = unsafe { GetModuleHandleW(None).unwrap() }.into();
            register_classes(instance);
            let hwnd = overlay_window(instance).unwrap();
            let kit = Kit::new().unwrap();
            let surface = Surface::new(&kit, 200, 120).unwrap();
            let frames = Box::new(FrameRing::ZERO);
            let stats = Box::new(Stats::ZERO);
            let view = View {
                connected: false,
                syncing: false,
                pid: 0,
                frames: &frames,
                stats: &stats,
                causes: &[],
                hotkey: true,
                demo: true,
                blink: true,
            };
            surface
                .draw(&kit, 0.5, |p| panels::left(p, 200.0, &view))
                .unwrap();
            assert!(
                surface.pixels().chunks(4).any(|px| px[3] > 0),
                "something was drawn"
            );
            surface
                .show(
                    hwnd,
                    POINT {
                        x: -20_000,
                        y: -20_000,
                    },
                )
                .unwrap();
            // SAFETY: ours.
            unsafe {
                assert!(!IsWindowVisible(hwnd).as_bool(), "never shown");
                let _ = DestroyWindow(hwnd);
            }
            assert_eq!(scale_for(1080), 1.0);
            assert_eq!(scale_for(100), 0.6);
        }
    }
}
