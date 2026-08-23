use crate::domain::{Item, ItemKind, to_wide, to_wide_slice};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::*;
use windows_numerics::Vector2;

pub mod metrics {
    pub const HEADER_HEIGHT: i32 = 56;
    pub const ITEM_HEIGHT: i32 = 54;
    pub const LIST_TOP: f32 = 64.0;
    pub const INPUT_X: f32 = 48.0;
    pub const ADMIN_ZONE_FAR: i32 = 176;
    pub const ADMIN_ZONE_NEAR: i32 = 82;
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub width: i32,
    pub opacity: f32,
    pub pill_radius: f32,
    pub badge_radius: f32,
    pub button_radius: f32,
}

impl Theme {
    pub fn from_config(config: &crate::config::Config) -> Self {
        let r = config.corner_radius;
        Self {
            width: config.width,
            opacity: config.opacity,
            pill_radius: r,
            badge_radius: (r * 0.85).max(0.0),
            button_radius: (r * 0.60).max(0.0),
        }
    }
}

struct BadgeGlyphs {
    nerd: PCWSTR,
    fluent: PCWSTR,
}

const ICON_FX: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f1ec}"),
    fluent: w!("\u{e1d0}"),
};
const ICON_CMD: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f120}"),
    fluent: w!("\u{e756}"),
};
const ICON_CFG: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f013}"),
    fluent: w!("\u{e713}"),
};
const ICON_APP: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f009}"),
    fluent: w!("\u{e71d}"),
};
const ICON_EXIT: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f011}"),
    fluent: w!("\u{e7e8}"),
};
const ICON_WEB: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f0ac}"),
    fluent: w!("\u{e774}"),
};
const ICON_PATH: BadgeGlyphs = BadgeGlyphs {
    nerd: w!("\u{f07b}"),
    fluent: w!("\u{e8b7}"),
};

fn pick_glyph(g: &BadgeGlyphs, is_nerd: bool) -> &[u16] {
    unsafe {
        if is_nerd {
            g.nerd.as_wide()
        } else {
            g.fluent.as_wide()
        }
    }
}
const KEY_CAP_COPY: PCWSTR = w!("↵ Copy");
const KEY_CAP_RUN: PCWSTR = w!("↵ Run");
const KEY_CAP_OPEN: PCWSTR = w!("↵ Open");
const KEY_CAP_EDIT: PCWSTR = w!("↵ Edit");
const KEY_CAP_EXIT: PCWSTR = w!("↵ Exit");
const KEY_CAP_ADMIN: PCWSTR = w!("Shift+↵ Admin");

pub fn window_scale(hwnd: HWND) -> f32 {
    (unsafe { GetDpiForWindow(hwnd) as f32 }) / 96.0
}

fn family_exists(dwrite_factory: &IDWriteFactory, family: PCWSTR) -> bool {
    unsafe {
        let mut collection: Option<IDWriteFontCollection> = None;
        if dwrite_factory
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return false;
        }
        let Some(collection) = collection else {
            return false;
        };
        let mut index = 0u32;
        let mut exists = BOOL(0);
        collection
            .FindFamilyName(family, &mut index, &mut exists)
            .is_ok()
            && exists.as_bool()
    }
}

fn probe_nerd_font_support(dwrite_factory: &IDWriteFactory, font_family: &str) -> bool {
    let lower = font_family.to_lowercase();
    if lower.contains("nerd") || lower.contains(" nf") || lower.ends_with("nf") {
        return true;
    }

    unsafe {
        if !family_exists(dwrite_factory, PCWSTR(to_wide(font_family).as_ptr())) {
            return false;
        }

        let mut collection: Option<IDWriteFontCollection> = None;
        if dwrite_factory
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return false;
        }
        let Some(collection) = collection else {
            return false;
        };
        let family_w = to_wide(font_family);
        let mut index = 0u32;
        let mut exists = BOOL(0);
        if collection
            .FindFamilyName(PCWSTR(family_w.as_ptr()), &mut index, &mut exists)
            .is_err()
            || !exists.as_bool()
        {
            return false;
        }

        let Ok(family) = collection.GetFontFamily(index) else {
            return false;
        };
        let Ok(font) = family.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        ) else {
            return false;
        };

        let Ok(font_face) = font.CreateFontFace() else {
            return false;
        };

        let test_codepoints = [0xF120u32, 0xF013u32];
        let mut glyph_indices = [0u16; 2];
        if font_face
            .GetGlyphIndices(
                test_codepoints.as_ptr(),
                test_codepoints.len() as u32,
                glyph_indices.as_mut_ptr(),
            )
            .is_err()
        {
            return false;
        }

        glyph_indices.iter().all(|&idx| idx != 0)
    }
}

pub struct IconCache {
    wic_factory: IWICImagingFactory,
    cache: HashMap<Arc<str>, Option<ID2D1Bitmap>>,
    loading: HashSet<Arc<str>>,
}

impl IconCache {
    pub fn new() -> Result<Self> {
        let wic_factory: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self {
            wic_factory,
            cache: HashMap::new(),
            loading: HashSet::new(),
        })
    }

    /// # Safety
    ///
    /// `rt` must be a valid and initialized Direct2D render target on the UI thread.
    pub unsafe fn get_or_load(
        &mut self,
        rt: &ID2D1RenderTarget,
        path: &Arc<str>,
        px: u32,
    ) -> Option<ID2D1Bitmap> {
        let key: Arc<str> = Arc::from(format!("{path}\u{0}{px}").as_str());
        if let Some(bm) = self.cache.get(&key) {
            return bm.clone();
        }
        if self.loading.contains(&key) {
            return None;
        }
        self.loading.insert(key.clone());
        let loaded = unsafe { self.load_shell_icon(rt, path, px) };
        self.loading.remove(&key);
        self.cache.insert(key, loaded.clone());
        if self.cache.len() > 512 {
            self.cache.clear();
        }
        loaded
    }

    unsafe fn load_shell_icon(
        &self,
        rt: &ID2D1RenderTarget,
        path: &str,
        _px: u32,
    ) -> Option<ID2D1Bitmap> {
        unsafe {
            let path_w = to_wide(path);
            let mut sfi = SHFILEINFOW::default();

            let res = SHGetFileInfoW(
                PCWSTR(path_w.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut sfi),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );

            if res == 0 || sfi.hIcon.is_invalid() {
                return None;
            }

            let wic_bitmap = self.wic_factory.CreateBitmapFromHICON(sfi.hIcon).ok()?;
            let _ = DestroyIcon(sfi.hIcon);

            let converter = self.wic_factory.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &wic_bitmap,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .ok()?;

            rt.CreateBitmapFromWicBitmap(&converter, None).ok()
        }
    }
}

/// # Safety
///
/// All D2D/DWrite parameters must be valid objects on the UI thread.
unsafe fn draw_badge_icon(
    target: &ID2D1RenderTarget,
    dwrite_factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
    text: &[u16],
    rect: &D2D_RECT_F,
    is_nerd_font: bool,
) {
    unsafe {
        let box_w = rect.right - rect.left;
        let box_h = rect.bottom - rect.top;

        let Ok(layout) = dwrite_factory.CreateTextLayout(text, format, box_w, box_h) else {
            return;
        };

        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = layout.GetMetrics(&mut m);

        let draw_x = if is_nerd_font {
            rect.left + (box_w - m.width) / 2.0 - m.left - 1.0
        } else {
            rect.left + (box_w - m.width) / 2.0 - m.left
        };

        let draw_y = if is_nerd_font {
            rect.top + (box_h - m.height) / 2.0 + 1.2
        } else {
            rect.top + (box_h - m.height) / 2.0
        };

        target.DrawTextLayout(
            Vector2::new(draw_x, draw_y),
            &layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }
}

/// # Safety
///
/// All brush/format/target parameters must be valid D2D objects on the UI thread.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_badge(
    target: &ID2D1RenderTarget,
    dwrite_factory: &IDWriteFactory,
    icon_fmt: &IDWriteTextFormat,
    rect: &D2D_RECT_F,
    radius: f32,
    bg: &ID2D1SolidColorBrush,
    border: &ID2D1SolidColorBrush,
    glyph: &[u16],
    glyph_brush: &ID2D1SolidColorBrush,
    is_nerd_font: bool,
) {
    let badge = D2D1_ROUNDED_RECT {
        rect: *rect,
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        target.FillRoundedRectangle(&badge, bg);
        target.DrawRoundedRectangle(&badge, border, 1.0, None);
        draw_badge_icon(
            target,
            dwrite_factory,
            icon_fmt,
            glyph_brush,
            glyph,
            rect,
            is_nerd_font,
        );
    }
}

struct BrushSet {
    text: ID2D1SolidColorBrush,
    subtext: ID2D1SolidColorBrush,
    selection: ID2D1SolidColorBrush,
    selection_border: ID2D1SolidColorBrush,
    hover: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    accent_subtle: ID2D1SolidColorBrush,
    accent_border: ID2D1SolidColorBrush,
    border: ID2D1SolidColorBrush,
    divider: ID2D1SolidColorBrush,
    badge_bg: ID2D1SolidColorBrush,
    badge_border: ID2D1SolidColorBrush,
    admin_badge: ID2D1SolidColorBrush,
}

struct FormatSet {
    input: IDWriteTextFormat,
    item_title: IDWriteTextFormat,
    item_sub: IDWriteTextFormat,
    badge: IDWriteTextFormat,
    badge_icon: IDWriteTextFormat,
}

pub struct D2DContext {
    target: ID2D1HwndRenderTarget,
    brushes: BrushSet,
    formats: FormatSet,
    applied_size: D2D_SIZE_U,
    is_nerd_font: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    pub current: f32,
    pub target: f32,
    pub velocity: f32,
    response: f32,
    damping: f32,
}

impl Spring {
    pub const fn new(initial: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            response: 0.10,
            damping: 0.70,
        }
    }

    pub fn new_slow(initial: f32, response: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            response,
            damping: 0.70,
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn reset(&mut self, val: f32) {
        self.current = val;
        self.target = val;
        self.velocity = 0.0;
    }

    pub fn update(&mut self, dt: f32) -> bool {
        let diff = self.target - self.current;
        if diff.abs() < 0.1 && self.velocity.abs() < 0.1 {
            self.current = self.target;
            self.velocity = 0.0;
            return false;
        }

        let sub_dt = dt / 4.0;
        let k = (2.0 * std::f32::consts::PI / self.response).powi(2);
        let c = 2.0 * self.damping * k.sqrt();

        for _ in 0..4 {
            let force = k * (self.target - self.current) - c * self.velocity;
            self.velocity += force * sub_dt;
            self.current += self.velocity * sub_dt;
        }
        true
    }
}

pub struct Renderer {
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    pub icon_cache: IconCache,
    font_family: String,
    theme: Theme,
    context: Option<D2DContext>,
}

impl Renderer {
    pub fn new(font_family: String) -> Result<Self> {
        unsafe {
            let d2d_factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_cache = IconCache::new()?;

            Ok(Self {
                d2d_factory,
                dwrite_factory,
                icon_cache,
                font_family,
                theme: Theme::from_config(&crate::config::Config::default()),
                context: None,
            })
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// # Safety
    ///
    /// `hwnd` must be a valid top-level window handle on the UI thread.
    pub unsafe fn ensure_context(&mut self, hwnd: HWND) -> Result<&mut D2DContext> {
        if self.context.is_none() {
            self.context = Some(unsafe { self.create_context(hwnd)? });
        }
        Ok(self.context.as_mut().unwrap())
    }

    pub fn invalidate(&mut self) {
        self.context = None;
        self.icon_cache.cache.clear();
    }

    pub fn set_font_family(&mut self, font_family: String) {
        if self.font_family != font_family {
            self.font_family = font_family;
            self.invalidate();
        }
    }

    unsafe fn create_context(&self, hwnd: HWND) -> Result<D2DContext> {
        unsafe {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);

            let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U {
                    width: (rect.right - rect.left) as u32,
                    height: (rect.bottom - rect.top) as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };

            let dpi = GetDpiForWindow(hwnd) as f32;
            let rt_properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi,
                dpiY: dpi,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };

            let target = self
                .d2d_factory
                .CreateHwndRenderTarget(&rt_properties, &hwnd_properties)?;
            let rt: ID2D1RenderTarget = target.cast()?;

            let mk = |r: f32, g: f32, b: f32, a: f32| {
                rt.CreateSolidColorBrush(&D2D1_COLOR_F { r, g, b, a }, None)
            };

            let brushes = BrushSet {
                text: mk(0.96, 0.96, 0.98, 0.98)?,
                subtext: mk(0.60, 0.63, 0.70, 0.85)?,
                selection: mk(1.0, 1.0, 1.0, 0.16)?,
                selection_border: mk(1.0, 1.0, 1.0, 0.26)?,
                hover: mk(1.0, 1.0, 1.0, 0.08)?,
                accent: mk(0.25, 0.58, 1.0, 1.0)?,
                accent_subtle: mk(0.25, 0.58, 1.0, 0.20)?,
                accent_border: mk(0.25, 0.58, 1.0, 0.45)?,
                border: mk(1.0, 1.0, 1.0, 0.14)?,
                divider: mk(1.0, 1.0, 1.0, 0.08)?,
                badge_bg: mk(1.0, 1.0, 1.0, 0.08)?,
                badge_border: mk(1.0, 1.0, 1.0, 0.16)?,
                admin_badge: mk(1.0, 0.72, 0.20, 1.0)?,
            };

            let mk_format = |family: &str,
                             weight: DWRITE_FONT_WEIGHT,
                             size: f32|
             -> Result<IDWriteTextFormat> {
                let try_family = |name: PCWSTR| {
                    self.dwrite_factory.CreateTextFormat(
                        name,
                        None,
                        weight,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        w!("zh-cn"),
                    )
                };
                let family_w = to_wide(family);
                try_family(PCWSTR(family_w.as_ptr()))
                    .or_else(|_| try_family(PCWSTR(w!("Segoe UI").as_ptr())))
            };

            let is_nerd_font = probe_nerd_font_support(&self.dwrite_factory, &self.font_family);

            let badge_icon_format = if is_nerd_font {
                mk_format(&self.font_family, DWRITE_FONT_WEIGHT_NORMAL, 13.5)?
            } else {
                let candidates = [
                    w!("Segoe Fluent Icons"),
                    w!("Segoe MDL2 Assets"),
                    w!("Segoe UI Symbol"),
                ];
                let picked = candidates
                    .iter()
                    .find(|f| family_exists(&self.dwrite_factory, **f))
                    .unwrap_or(&candidates[2]);
                self.dwrite_factory.CreateTextFormat(
                    *picked,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    13.5,
                    w!("en-us"),
                )?
            };

            let formats = FormatSet {
                input: mk_format(&self.font_family, DWRITE_FONT_WEIGHT_NORMAL, 18.0)?,
                item_title: mk_format(&self.font_family, DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0)?,
                item_sub: mk_format(&self.font_family, DWRITE_FONT_WEIGHT_NORMAL, 11.5)?,
                badge: mk_format(&self.font_family, DWRITE_FONT_WEIGHT_SEMI_BOLD, 10.5)?,
                badge_icon: badge_icon_format,
            };

            let _ = formats.input.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = formats
                .item_title
                .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = formats
                .item_sub
                .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = formats.badge.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = formats
                .badge_icon
                .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);

            let _ = formats
                .input
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = formats
                .item_title
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = formats
                .item_sub
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = formats
                .badge
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = formats.badge.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);

            let trimming_options = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let trimming_sign = self
                .dwrite_factory
                .CreateEllipsisTrimmingSign(&formats.item_sub)
                .ok();
            if let Some(sign) = &trimming_sign {
                let _ = formats
                    .item_title
                    .SetTrimming(&trimming_options, Some(sign));
                let _ = formats.item_sub.SetTrimming(&trimming_options, Some(sign));
            }

            Ok(D2DContext {
                target,
                brushes,
                formats,
                applied_size: D2D_SIZE_U {
                    width: (rect.right - rect.left) as u32,
                    height: (rect.bottom - rect.top) as u32,
                },
                is_nerd_font,
            })
        }
    }

    /// # Safety
    ///
    /// `hwnd` must be an initialized and active window handle on the UI thread.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn render(
        &mut self,
        hwnd: HWND,
        query: &str,
        placeholder: &str,
        items: &[&Item],
        selected: usize,
        caret_visible: bool,
        hovered: Option<usize>,
        pill_y: f32,
    ) -> Result<()> {
        unsafe {
            self.ensure_context(hwnd)?;
            let icon_px = (32.0 * GetDpiForWindow(hwnd) as f32 / 96.0).round() as u32;
            let theme = self.theme;
            let dwrite_factory = self.dwrite_factory.clone();
            let Renderer {
                icon_cache,
                context,
                ..
            } = self;
            let ctx = context.as_mut().unwrap();
            let is_nerd_font = ctx.is_nerd_font;

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let size = D2D_SIZE_U {
                width: (rect.right - rect.left) as u32,
                height: (rect.bottom - rect.top) as u32,
            };
            if ctx.applied_size != size {
                let _ = ctx.target.Resize(&size);
                ctx.applied_size = size;
            }

            let target: ID2D1RenderTarget = ctx.target.cast()?;
            target.BeginDraw();

            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.11,
                g: 0.11,
                b: 0.14,
                a: theme.opacity,
            }));

            let win_rect = D2D_RECT_F {
                left: 0.5,
                top: 0.5,
                right: size.width as f32 - 0.5,
                bottom: size.height as f32 - 0.5,
            };
            target.DrawRectangle(&win_rect, &ctx.brushes.border, 1.0, None);

            let sub_brush = &ctx.brushes.subtext;
            let mag_center = Vector2::new(28.0, 28.0);
            let mag_ellipse = D2D1_ELLIPSE {
                point: mag_center,
                radiusX: 6.0,
                radiusY: 6.0,
            };
            target.DrawEllipse(&mag_ellipse, sub_brush, 1.8, None);
            target.DrawLine(
                Vector2::new(32.5, 32.5),
                Vector2::new(38.0, 38.0),
                sub_brush,
                2.0,
                None,
            );

            let input_clip_left = metrics::INPUT_X;
            let input_clip_right = theme.width as f32 - 24.0;
            let max_visible_width = input_clip_right - input_clip_left;

            let (text_to_draw, is_placeholder) = if query.is_empty() {
                (placeholder, true)
            } else {
                (query, false)
            };
            let q_wide = to_wide_slice(text_to_draw);

            let mut caret_offset_x = 0.0;
            if !is_placeholder
                && let Ok(layout) = dwrite_factory.CreateTextLayout(
                    &q_wide,
                    &ctx.formats.input,
                    10000.0,
                    metrics::HEADER_HEIGHT as f32,
                )
            {
                let mut x = 0.0;
                let mut y = 0.0;
                let mut hit_metrics = DWRITE_HIT_TEST_METRICS::default();
                let _ = layout.HitTestTextPosition(
                    q_wide.len() as u32,
                    false,
                    &mut x,
                    &mut y,
                    &mut hit_metrics,
                );
                caret_offset_x = x;
            }

            let scroll_x = if caret_offset_x > max_visible_width {
                caret_offset_x - max_visible_width
            } else {
                0.0
            };

            let input_rect = D2D_RECT_F {
                left: input_clip_left - scroll_x,
                top: 0.0,
                right: input_clip_left - scroll_x + 10000.0,
                bottom: metrics::HEADER_HEIGHT as f32,
            };

            let text_brush = if is_placeholder {
                &ctx.brushes.subtext
            } else {
                &ctx.brushes.text
            };

            let clip_rect = D2D_RECT_F {
                left: input_clip_left,
                top: 0.0,
                right: input_clip_right,
                bottom: metrics::HEADER_HEIGHT as f32,
            };

            target.PushAxisAlignedClip(&clip_rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            target.DrawText(
                &q_wide,
                &ctx.formats.input,
                &input_rect,
                text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            target.PopAxisAlignedClip();

            if caret_visible {
                let final_caret_x = input_clip_left + caret_offset_x - scroll_x;
                target.DrawLine(
                    Vector2::new(final_caret_x, 16.0),
                    Vector2::new(final_caret_x, 40.0),
                    &ctx.brushes.accent,
                    2.0,
                    None,
                );
            }

            if !items.is_empty() {
                let divider_y = metrics::HEADER_HEIGHT as f32;
                target.DrawLine(
                    Vector2::new(0.0, divider_y),
                    Vector2::new(size.width as f32, divider_y),
                    &ctx.brushes.divider,
                    1.0,
                    None,
                );
            }

            let start_y = metrics::LIST_TOP;
            let item_h = metrics::ITEM_HEIGHT as f32;

            if !items.is_empty() {
                let pill_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 8.0,
                        top: pill_y,
                        right: size.width as f32 - 8.0,
                        bottom: pill_y + item_h - 4.0,
                    },
                    radiusX: theme.pill_radius,
                    radiusY: theme.pill_radius,
                };
                target.FillRoundedRectangle(&pill_rect, &ctx.brushes.selection);
                target.DrawRoundedRectangle(&pill_rect, &ctx.brushes.selection_border, 1.0, None);
            }

            for (i, item) in items.iter().enumerate() {
                let top = start_y + (i as f32) * item_h;
                let bottom = top + item_h - 4.0;

                let item_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 8.0,
                        top,
                        right: size.width as f32 - 8.0,
                        bottom,
                    },
                    radiusX: theme.pill_radius,
                    radiusY: theme.pill_radius,
                };

                if Some(i) == hovered && i != selected {
                    target.FillRoundedRectangle(&item_rect, &ctx.brushes.hover);
                }

                let is_calc = matches!(item.kind, ItemKind::Calculator { .. });
                let is_cmd = matches!(item.kind, ItemKind::Command { .. });
                let is_cfg = matches!(item.kind, ItemKind::Config);
                let is_exit = matches!(item.kind, ItemKind::Exit);

                let icon_container = D2D_RECT_F {
                    left: 20.0,
                    top: top + 9.0,
                    right: 52.0,
                    bottom: top + 41.0,
                };
                let icon_fmt = &ctx.formats.badge_icon;
                let badge_fmt = &ctx.formats.badge;

                match &item.kind {
                    ItemKind::Config => draw_badge(
                        &target,
                        &dwrite_factory,
                        icon_fmt,
                        &icon_container,
                        theme.badge_radius,
                        &ctx.brushes.accent_subtle,
                        &ctx.brushes.accent_border,
                        pick_glyph(&ICON_CFG, is_nerd_font),
                        &ctx.brushes.accent,
                        is_nerd_font,
                    ),
                    ItemKind::Exit => draw_badge(
                        &target,
                        &dwrite_factory,
                        icon_fmt,
                        &icon_container,
                        theme.badge_radius,
                        &ctx.brushes.badge_bg,
                        &ctx.brushes.border,
                        pick_glyph(&ICON_EXIT, is_nerd_font),
                        &ctx.brushes.text,
                        is_nerd_font,
                    ),
                    ItemKind::Calculator { .. } => draw_badge(
                        &target,
                        &dwrite_factory,
                        icon_fmt,
                        &icon_container,
                        theme.badge_radius,
                        &ctx.brushes.accent_subtle,
                        &ctx.brushes.accent_border,
                        pick_glyph(&ICON_FX, is_nerd_font),
                        &ctx.brushes.accent,
                        is_nerd_font,
                    ),
                    ItemKind::Command { .. } => draw_badge(
                        &target,
                        &dwrite_factory,
                        icon_fmt,
                        &icon_container,
                        theme.badge_radius,
                        &ctx.brushes.badge_bg,
                        &ctx.brushes.border,
                        pick_glyph(&ICON_CMD, is_nerd_font),
                        &ctx.brushes.admin_badge,
                        is_nerd_font,
                    ),
                    ItemKind::Web => draw_badge(
                        &target,
                        &dwrite_factory,
                        icon_fmt,
                        &icon_container,
                        theme.badge_radius,
                        &ctx.brushes.accent_subtle,
                        &ctx.brushes.accent_border,
                        pick_glyph(&ICON_WEB, is_nerd_font),
                        &ctx.brushes.accent,
                        is_nerd_font,
                    ),
                    ItemKind::Path => draw_badge(
                        &target,
                        &dwrite_factory,
                        icon_fmt,
                        &icon_container,
                        theme.badge_radius,
                        &ctx.brushes.badge_bg,
                        &ctx.brushes.border,
                        pick_glyph(&ICON_PATH, is_nerd_font),
                        &ctx.brushes.subtext,
                        is_nerd_font,
                    ),
                    ItemKind::Application => {
                        let icon_bmp = icon_cache.get_or_load(&target, &item.path, icon_px);
                        if let Some(bmp) = icon_bmp {
                            target.DrawBitmap(
                                &bmp,
                                Some(&icon_container),
                                1.0,
                                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                                None,
                            );
                        } else {
                            draw_badge(
                                &target,
                                &dwrite_factory,
                                icon_fmt,
                                &icon_container,
                                theme.badge_radius,
                                &ctx.brushes.badge_bg,
                                &ctx.brushes.border,
                                pick_glyph(&ICON_APP, is_nerd_font),
                                &ctx.brushes.subtext,
                                is_nerd_font,
                            );
                        }
                    }
                }

                let text_max_right = size.width as f32 - 185.0;

                let title_w = to_wide_slice(&item.name);
                let title_brush = if is_calc || is_cfg {
                    &ctx.brushes.accent
                } else if is_cmd {
                    &ctx.brushes.admin_badge
                } else {
                    &ctx.brushes.text
                };
                target.DrawText(
                    &title_w,
                    &ctx.formats.item_title,
                    &D2D_RECT_F {
                        left: 62.0,
                        top: top + 4.0,
                        right: text_max_right,
                        bottom: top + 27.0,
                    },
                    title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                let sub_w = to_wide_slice(&item.path);
                target.DrawText(
                    &sub_w,
                    &ctx.formats.item_sub,
                    &D2D_RECT_F {
                        left: 62.0,
                        top: top + 27.0,
                        right: text_max_right,
                        bottom: top + 46.0,
                    },
                    &ctx.brushes.subtext,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                if i == selected {
                    let action_str = if is_calc {
                        KEY_CAP_COPY
                    } else if is_cmd {
                        KEY_CAP_RUN
                    } else if is_cfg {
                        KEY_CAP_EDIT
                    } else if is_exit {
                        KEY_CAP_EXIT
                    } else {
                        KEY_CAP_OPEN
                    };

                    let action_rect = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: size.width as f32 - 76.0,
                            top: top + 13.5,
                            right: size.width as f32 - 16.0,
                            bottom: top + 36.5,
                        },
                        radiusX: theme.button_radius,
                        radiusY: theme.button_radius,
                    };
                    target.FillRoundedRectangle(&action_rect, &ctx.brushes.badge_bg);
                    target.DrawRoundedRectangle(&action_rect, &ctx.brushes.badge_border, 1.0, None);
                    target.DrawText(
                        action_str.as_wide(),
                        badge_fmt,
                        &action_rect.rect,
                        &ctx.brushes.text,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );

                    if !is_calc && !is_exit && !is_cfg {
                        let admin_w = KEY_CAP_ADMIN.as_wide();
                        let admin_rect = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: size.width as f32 - metrics::ADMIN_ZONE_FAR as f32,
                                top: top + 13.5,
                                right: size.width as f32 - metrics::ADMIN_ZONE_NEAR as f32,
                                bottom: top + 36.5,
                            },
                            radiusX: theme.button_radius,
                            radiusY: theme.button_radius,
                        };
                        target.FillRoundedRectangle(&admin_rect, &ctx.brushes.badge_bg);
                        target.DrawRoundedRectangle(
                            &admin_rect,
                            &ctx.brushes.badge_border,
                            1.0,
                            None,
                        );
                        target.DrawText(
                            admin_w,
                            badge_fmt,
                            &admin_rect.rect,
                            &ctx.brushes.subtext,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
            }

            if target.EndDraw(None, None).is_err() {
                self.context = None;
                icon_cache.cache.clear();
            }
        }
        Ok(())
    }
}
