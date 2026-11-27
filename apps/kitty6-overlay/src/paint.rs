//! Drawing: a 32-bit premultiplied DIB per overlay window, Direct2D (the
//! software rasterizer: no card memory, no GPU time taken from the game)
//! drawing into it through a DC render target, DirectWrite for text, and
//! the small vocabulary the panels are made of.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{Result, w};
use windows_numerics::Vector2;

/// A colour, straight alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

impl Rgba {
    pub const fn hex(rgb: u32, a: f32) -> Rgba {
        Rgba(
            ((rgb >> 16) & 0xff) as f32 / 255.0,
            ((rgb >> 8) & 0xff) as f32 / 255.0,
            (rgb & 0xff) as f32 / 255.0,
            a,
        )
    }
    pub fn alpha(self, a: f32) -> Rgba {
        Rgba(self.0, self.1, self.2, a)
    }
    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: self.0,
            g: self.1,
            b: self.2,
            a: self.3,
        }
    }
}

pub const CYAN: Rgba = Rgba::hex(0x00f0ff, 1.0);
pub const GREEN: Rgba = Rgba::hex(0x39ff8f, 1.0);
pub const MAGENTA: Rgba = Rgba::hex(0xff2bd6, 1.0);
pub const AMBER: Rgba = Rgba::hex(0xffbf1a, 1.0);
pub const RED: Rgba = Rgba::hex(0xff3b4e, 1.0);
pub const VIOLET: Rgba = Rgba::hex(0x9a7bff, 1.0);
pub const TEXT: Rgba = Rgba::hex(0xcdeff5, 1.0);
pub const DIM: Rgba = Rgba::hex(0x6a9aa5, 1.0);
pub const PANEL: Rgba = Rgba::hex(0x040b12, 0.74);

/// The colour of a frame-time severity (`model::severity`).
pub fn severity_colour(s: u8) -> Rgba {
    [GREEN, CYAN, AMBER, MAGENTA][usize::from(s.min(3))]
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Right,
    Center,
}

/// A text style: size in pixels at scale 1, bold, wrapping.
#[derive(Clone, Copy)]
pub struct Font {
    pub size: f32,
    pub bold: bool,
    pub wrap: bool,
}

pub const TINY: Font = Font {
    size: 9.5,
    bold: true,
    wrap: false,
};
pub const SMALL: Font = Font {
    size: 11.0,
    bold: false,
    wrap: false,
};
pub const SMALL_WRAP: Font = Font {
    size: 10.5,
    bold: false,
    wrap: true,
};
pub const BODY_BOLD: Font = Font {
    size: 12.5,
    bold: true,
    wrap: false,
};
pub const MEDIUM: Font = Font {
    size: 16.0,
    bold: true,
    wrap: false,
};
pub const LARGE: Font = Font {
    size: 22.0,
    bold: true,
    wrap: false,
};
pub const HUGE: Font = Font {
    size: 44.0,
    bold: true,
    wrap: false,
};

/// The shared factories.
pub struct Kit {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    dash: ID2D1StrokeStyle,
    formats: RefCell<HashMap<(u32, bool, bool, Align), IDWriteTextFormat>>,
}

impl Kit {
    pub fn new() -> Result<Kit> {
        // SAFETY: plain factory creation.
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let props = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_MITER,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_CUSTOM,
                dashOffset: 0.0,
            };
            let dash = d2d.CreateStrokeStyle(&props, Some(&[3.0, 3.0]))?;
            Ok(Kit {
                d2d,
                dwrite,
                dash,
                formats: RefCell::new(HashMap::new()),
            })
        }
    }

    fn format(&self, font: Font, scale: f32, align: Align) -> Option<IDWriteTextFormat> {
        let px = (font.size * scale * 4.0).round() as u32; // quarter pixels
        let key = (px, font.bold, font.wrap, align);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Some(f.clone());
        }
        // SAFETY: plain creation and settings of a text format.
        let f = unsafe {
            let f = self
                .dwrite
                .CreateTextFormat(
                    w!("Consolas"),
                    None,
                    if font.bold {
                        DWRITE_FONT_WEIGHT_BOLD
                    } else {
                        DWRITE_FONT_WEIGHT_NORMAL
                    },
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    px as f32 / 4.0,
                    w!("en-us"),
                )
                .ok()?;
            let _ = f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            });
            let _ = f.SetWordWrapping(if font.wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            });
            f
        };
        self.formats.borrow_mut().insert(key, f.clone());
        Some(f)
    }
}

/// A window's pixels: a top-down 32-bit DIB in a memory DC, and a render
/// target bound to it.
pub struct Surface {
    pub width: i32,
    pub height: i32,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    target: ID2D1DCRenderTarget,
}

impl Surface {
    pub fn new(kit: &Kit, width: i32, height: i32) -> Result<Surface> {
        let (width, height) = (width.max(1), height.max(1));
        // SAFETY: a DIB section selected into a DC of ours; freed in Drop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(dc, bitmap.into());
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = kit.d2d.CreateDCRenderTarget(&props)?;
            let rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            target.BindDC(dc, &rect)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Ok(Surface {
                width,
                height,
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                target,
            })
        }
    }

    /// Draws with `f` onto a cleared surface.
    pub fn draw(&self, kit: &Kit, scale: f32, f: impl FnOnce(&Painter)) -> Result<()> {
        // SAFETY: our target, bound to our DC.
        unsafe {
            self.target.BeginDraw();
            self.target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            let brush = self.target.CreateSolidColorBrush(&TEXT.d2d(), None)?;
            let painter = Painter {
                kit,
                target: &self.target,
                brush,
                scale,
            };
            f(&painter);
            self.target.EndDraw(None, None)
        }
    }

    /// Shows the pixels as `window`'s, at `at` on the screen.
    pub fn show(&self, window: windows::Win32::Foundation::HWND, at: POINT) -> Result<()> {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE {
            cx: self.width,
            cy: self.height,
        };
        let from = POINT { x: 0, y: 0 };
        // SAFETY: a layered window of ours and our DC.
        unsafe {
            UpdateLayeredWindow(
                window,
                None,
                Some(&at),
                Some(&size),
                Some(self.dc),
                Some(&from),
                windows::Win32::Foundation::COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }

    /// The premultiplied BGRA pixels, rows top down.
    pub fn pixels(&self) -> &[u8] {
        // SAFETY: the DIB's bits, `width * height * 4` bytes, alive with us;
        // GDI is done with them once a draw returns.
        unsafe {
            std::slice::from_raw_parts(self.bits, self.width as usize * self.height as usize * 4)
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: ours.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// What panels draw with. Coordinates are pixels; `scale` sizes text and
/// strokes.
pub struct Painter<'a> {
    kit: &'a Kit,
    target: &'a ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    pub scale: f32,
}

fn v(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

impl Painter<'_> {
    fn colour(&self, c: Rgba) -> &ID2D1SolidColorBrush {
        // SAFETY: our brush.
        unsafe { self.brush.SetColor(&c.d2d()) };
        &self.brush
    }

    pub fn fill(&self, r: Rect, c: Rgba) {
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.target.FillRectangle(&r.d2d(), self.colour(c)) };
    }

    pub fn stroke(&self, r: Rect, c: Rgba, width: f32) {
        // SAFETY: as above.
        unsafe {
            self.target
                .DrawRectangle(&r.d2d(), self.colour(c), width, None)
        };
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba, width: f32) {
        // SAFETY: as above.
        unsafe {
            self.target
                .DrawLine(v(x0, y0), v(x1, y1), self.colour(c), width, None)
        };
    }

    pub fn dashed(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba, width: f32) {
        // SAFETY: as above.
        unsafe {
            self.target
                .DrawLine(v(x0, y0), v(x1, y1), self.colour(c), width, &self.kit.dash)
        };
    }

    pub fn dot(&self, x: f32, y: f32, r: f32, c: Rgba) {
        let e = D2D1_ELLIPSE {
            point: v(x, y),
            radiusX: r,
            radiusY: r,
        };
        // SAFETY: as above.
        unsafe { self.target.FillEllipse(&e, self.colour(c)) };
    }

    pub fn text(&self, r: Rect, s: &str, font: Font, c: Rgba, align: Align) {
        let Some(format) = self.kit.format(font, self.scale, align) else {
            return;
        };
        let wide: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: as above.
        unsafe {
            self.target.DrawText(
                &wide,
                &format,
                &r.d2d(),
                self.colour(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    /// Text with a soft glow behind it.
    pub fn glow_text(&self, r: Rect, s: &str, font: Font, c: Rgba, align: Align) {
        let o = 1.2 * self.scale;
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o)] {
            self.text(
                Rect::new(r.x + dx, r.y + dy, r.w, r.h),
                s,
                font,
                c.alpha(0.18),
                align,
            );
        }
        self.text(r, s, font, c, align);
    }

    fn path(&self, points: &[(f32, f32)], closed_to: Option<f32>) -> Option<ID2D1PathGeometry> {
        let (first, rest) = points.split_first()?;
        // SAFETY: a geometry of ours, opened, filled and closed here.
        unsafe {
            let path = self.kit.d2d.CreatePathGeometry().ok()?;
            let sink = path.Open().ok()?;
            let pts: Vec<Vector2> = rest.iter().map(|&(x, y)| v(x, y)).collect();
            match closed_to {
                Some(base) => {
                    sink.BeginFigure(v(first.0, base), D2D1_FIGURE_BEGIN_FILLED);
                    sink.AddLines(&[v(first.0, first.1)]);
                    sink.AddLines(&pts);
                    sink.AddLines(&[v(points[points.len() - 1].0, base)]);
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                }
                None => {
                    sink.BeginFigure(v(first.0, first.1), D2D1_FIGURE_BEGIN_HOLLOW);
                    sink.AddLines(&pts);
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
            }
            sink.Close().ok()?;
            Some(path)
        }
    }

    /// A polyline with a glow under it.
    pub fn polyline(&self, points: &[(f32, f32)], c: Rgba, width: f32) {
        let Some(path) = self.path(points, None) else {
            return;
        };
        // SAFETY: as above.
        unsafe {
            self.target
                .DrawGeometry(&path, self.colour(c.alpha(0.16 * c.3)), width * 4.0, None);
            self.target.DrawGeometry(&path, self.colour(c), width, None);
        }
    }

    /// The area under a polyline down to `base`.
    pub fn area(&self, points: &[(f32, f32)], base: f32, c: Rgba) {
        let Some(path) = self.path(points, Some(base)) else {
            return;
        };
        // SAFETY: as above.
        unsafe { self.target.FillGeometry(&path, self.colour(c), None) };
    }

    pub fn clip(&self, r: Rect, f: impl FnOnce()) {
        // SAFETY: as above; pushed and popped in pairs.
        unsafe {
            self.target
                .PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED)
        };
        f();
        // SAFETY: as above.
        unsafe { self.target.PopAxisAlignedClip() };
    }

    // The HUD's vocabulary.

    /// A panel: translucent body, faint scanlines, a thin frame with bright
    /// corner brackets, a coded title tab. The area inside, under the title.
    pub fn panel(&self, r: Rect, code: &str, title: &str, accent: Rgba) -> Rect {
        let s = self.scale;
        self.fill(r, PANEL);
        let mut y = r.y + 2.0 * s;
        while y < r.bottom() {
            self.line(r.x, y, r.right(), y, CYAN.alpha(0.025), 1.0);
            y += 3.0 * s;
        }
        self.stroke(r.inset(0.5, 0.5), accent.alpha(0.28), 1.0);
        let k = 9.0 * s;
        let w = 1.8 * s;
        for (cx, cy, dx, dy) in [
            (r.x, r.y, 1.0, 1.0),
            (r.right(), r.y, -1.0, 1.0),
            (r.x, r.bottom(), 1.0, -1.0),
            (r.right(), r.bottom(), -1.0, -1.0),
        ] {
            self.line(cx, cy, cx + dx * k, cy, accent, w);
            self.line(cx, cy, cx, cy + dy * k, accent, w);
        }
        let tab = Rect::new(r.x + 8.0 * s, r.y + 5.0 * s, r.w - 16.0 * s, 13.0 * s);
        self.fill(Rect::new(tab.x, tab.y + 1.5 * s, 3.0 * s, 10.0 * s), accent);
        self.text(
            Rect::new(tab.x + 7.0 * s, tab.y, 40.0 * s, tab.h),
            code,
            TINY,
            accent.alpha(0.75),
            Align::Left,
        );
        self.text(
            Rect::new(tab.x + 30.0 * s, tab.y, tab.w - 30.0 * s, tab.h),
            title,
            TINY,
            TEXT.alpha(0.9),
            Align::Left,
        );
        self.line(
            tab.x,
            tab.bottom() + 3.0 * s,
            r.right() - 8.0 * s,
            tab.bottom() + 3.0 * s,
            accent.alpha(0.18),
            1.0,
        );
        Rect::new(
            r.x + 9.0 * s,
            tab.bottom() + 7.0 * s,
            r.w - 18.0 * s,
            r.bottom() - tab.bottom() - 12.0 * s,
        )
    }

    /// A faint grid: `rows` horizontal and `cols` vertical divisions.
    pub fn grid(&self, r: Rect, rows: u32, cols: u32) {
        for i in 0..=rows {
            let y = r.y + r.h * i as f32 / rows as f32;
            self.line(r.x, y, r.right(), y, CYAN.alpha(0.07), 1.0);
        }
        for i in 0..=cols {
            let x = r.x + r.w * i as f32 / cols as f32;
            self.line(x, r.y, x, r.bottom(), CYAN.alpha(0.05), 1.0);
        }
    }

    /// A labelled gauge bar: `label` left, `value` right, a segmented bar
    /// filled to `frac` (0 to 1), and an optional marker at `mark`.
    pub fn gauge(&self, r: Rect, label: &str, value: &str, frac: f32, c: Rgba, mark: Option<f32>) {
        let s = self.scale;
        let text_h = 12.0 * s;
        self.text(
            Rect::new(r.x, r.y, r.w * 0.55, text_h),
            label,
            TINY,
            DIM,
            Align::Left,
        );
        self.text(
            Rect::new(r.x + r.w * 0.3, r.y - 1.0 * s, r.w * 0.7, text_h + 2.0 * s),
            value,
            SMALL,
            c,
            Align::Right,
        );
        let bar = Rect::new(
            r.x,
            r.y + text_h + 2.0 * s,
            r.w,
            (r.h - text_h - 2.0 * s).max(3.0 * s),
        );
        self.fill(bar, CYAN.alpha(0.06));
        let frac = if frac.is_finite() {
            frac.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let filled = Rect::new(bar.x, bar.y, bar.w * frac, bar.h);
        self.fill(filled, c.alpha(0.55));
        self.fill(
            Rect::new(filled.right() - 2.0 * s, bar.y, 2.0 * s, bar.h),
            c,
        );
        // Segments.
        let mut x = bar.x;
        while x < bar.right() {
            self.line(x, bar.y, x, bar.bottom(), PANEL.alpha(0.9), 1.0);
            x += 6.0 * s;
        }
        if let Some(m) = mark.filter(|m| m.is_finite()) {
            let mx = bar.x + bar.w * m.clamp(0.0, 1.0);
            self.line(
                mx,
                bar.y - 2.0 * s,
                mx,
                bar.bottom() + 2.0 * s,
                RED,
                1.5 * s,
            );
        }
        self.stroke(bar, c.alpha(0.3), 1.0);
    }

    /// A sparkline of `values` (oldest first) scaled to `top`, with area fill.
    pub fn sparkline(&self, r: Rect, values: &[f32], top: f32, c: Rgba) {
        if values.len() < 2 {
            return;
        }
        let n = values.len() as f32 - 1.0;
        let points: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let val = if val.is_finite() { val } else { 0.0 };
                (
                    r.x + r.w * i as f32 / n,
                    r.bottom() - r.h * (val / top).clamp(0.0, 1.0),
                )
            })
            .collect();
        self.area(&points, r.bottom(), c.alpha(0.13));
        self.polyline(&points, c, 1.3 * self.scale);
        if let Some(&(x, y)) = points.last() {
            self.dot(x, y, 2.2 * self.scale, c);
        }
    }
}
