use crate::domain::{Item, ItemKind, to_wide, to_wide_slice};
use std::collections::HashMap;
use std::sync::Arc;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::*;

pub mod metrics {
    pub const WINDOW_WIDTH: i32 = 760;
    pub const HEADER_HEIGHT: i32 = 56;
    pub const ITEM_HEIGHT: i32 = 54;
    pub const LIST_TOP: f32 = 64.0;
}

const BADGE_FX: PCWSTR = w!("fx");
const BADGE_CMD: PCWSTR = w!(">_");
const BADGE_APP: PCWSTR = w!("⊞");
const KEY_CAP_COPY: PCWSTR = w!("↵ Copy");
const KEY_CAP_RUN: PCWSTR = w!("↵ Run");
const KEY_CAP_OPEN: PCWSTR = w!("↵ Open");
const KEY_CAP_ADMIN: PCWSTR = w!("Shift+↵ Admin");
const PLACEHOLDER: PCWSTR =
    w!("Search apps, commands (Enter to run), or type a formula (e.g. 2^10)...");

pub struct IconCache {
    wic_factory: IWICImagingFactory,
    cache: HashMap<Arc<str>, Option<ID2D1Bitmap>>,
}

impl IconCache {
    pub fn new() -> Result<Self> {
        let wic_factory: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        Ok(Self {
            wic_factory,
            cache: HashMap::new(),
        })
    }

    /// # Safety
    ///
    /// `rt` must be a valid and initialized Direct2D render target on the UI thread.
    pub unsafe fn get_or_load(
        &mut self,
        rt: &ID2D1RenderTarget,
        path: &Arc<str>,
    ) -> Option<ID2D1Bitmap> {
        if let Some(bm) = self.cache.get(path) {
            return bm.clone();
        }

        let loaded = unsafe { self.load_shell_icon(rt, path) };
        self.cache.insert(path.clone(), loaded.clone());
        loaded
    }

    unsafe fn load_shell_icon(&self, rt: &ID2D1RenderTarget, path: &str) -> Option<ID2D1Bitmap> {
        unsafe {
            let path_w = to_wide(path);
            let shell_item: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(path_w.as_ptr()), None).ok()?;
            let image_factory: IShellItemImageFactory = shell_item.cast().ok()?;

            let hbitmap = image_factory
                .GetImage(
                    SIZE { cx: 32, cy: 32 },
                    SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY,
                )
                .ok()?;

            let wic_bitmap =
                self.wic_factory
                    .CreateBitmapFromHBITMAP(hbitmap, None, WICBitmapUseAlpha);
            let _ = DeleteObject(hbitmap);

            let wic_bitmap = wic_bitmap.ok()?;
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
}

pub struct D2DContext {
    target: ID2D1HwndRenderTarget,
    brushes: BrushSet,
    formats: FormatSet,
    applied_size: D2D_SIZE_U,
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

    /// 采用 4 步子迭代，杜绝数值爆炸
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
    context: Option<D2DContext>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d_factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_cache = IconCache::new()?;

            Ok(Self {
                d2d_factory,
                dwrite_factory,
                icon_cache,
                context: None,
            })
        }
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

            let rt_properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 0.0,
                dpiY: 0.0,
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
                selection: mk(1.0, 1.0, 1.0, 0.16)?, // 清晰的选中高亮
                selection_border: mk(1.0, 1.0, 1.0, 0.26)?, // 清晰边框
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

            let formats = FormatSet {
                input: self.dwrite_factory.CreateTextFormat(
                    w!("Segoe UI Variable Display"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    19.0,
                    w!("zh-cn"),
                )?,
                item_title: self.dwrite_factory.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    14.5,
                    w!("zh-cn"),
                )?,
                item_sub: self.dwrite_factory.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    11.5,
                    w!("zh-cn"),
                )?,
                badge: self.dwrite_factory.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    10.5,
                    w!("zh-cn"),
                )?,
            };

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

            Ok(D2DContext {
                target,
                brushes,
                formats,
                applied_size: D2D_SIZE_U {
                    width: (rect.right - rect.left) as u32,
                    height: (rect.bottom - rect.top) as u32,
                },
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
        items: &[&Item],
        selected: usize,
        caret_visible: bool,
        hovered: Option<usize>,
        pill_y: f32,
    ) -> Result<()> {
        unsafe {
            self.ensure_context(hwnd)?;
            let dwrite_factory = self.dwrite_factory.clone();
            let Renderer {
                icon_cache,
                context,
                ..
            } = self;
            let ctx = context.as_mut().unwrap();

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

            // 关键：保留 72% Alpha，让 Mica / Acrylic 磨砂质感通透显现
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.11,
                g: 0.11,
                b: 0.14,
                a: 0.72,
            }));

            // 边框
            let win_rect = D2D_RECT_F {
                left: 0.5,
                top: 0.5,
                right: size.width as f32 - 0.5,
                bottom: size.height as f32 - 0.5,
            };
            target.DrawRectangle(&win_rect, &ctx.brushes.border, 1.0, None);

            // 放大镜
            let sub_brush = &ctx.brushes.subtext;
            let mag_center = D2D_POINT_2F { x: 28.0, y: 28.0 };
            let mag_ellipse = D2D1_ELLIPSE {
                point: mag_center,
                radiusX: 6.0,
                radiusY: 6.0,
            };
            target.DrawEllipse(&mag_ellipse, sub_brush, 1.8, None);
            target.DrawLine(
                D2D_POINT_2F { x: 32.5, y: 32.5 },
                D2D_POINT_2F { x: 38.0, y: 38.0 },
                sub_brush,
                2.0,
                None,
            );

            // 输入文字
            let q_wide = if query.is_empty() {
                PLACEHOLDER.as_wide().to_vec()
            } else {
                to_wide_slice(query)
            };
            let text_brush = if query.is_empty() {
                &ctx.brushes.subtext
            } else {
                &ctx.brushes.text
            };
            target.DrawText(
                &q_wide,
                &ctx.formats.input,
                &D2D_RECT_F {
                    left: 48.0,
                    top: 10.0,
                    right: (metrics::WINDOW_WIDTH - 20) as f32,
                    bottom: 46.0,
                },
                text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // 光标
            if caret_visible {
                let caret_x = if query.is_empty() {
                    48.0
                } else {
                    let q_slice = to_wide_slice(query);
                    match dwrite_factory.CreateTextLayout(
                        &q_slice,
                        &ctx.formats.input,
                        (metrics::WINDOW_WIDTH - 68) as f32,
                        36.0,
                    ) {
                        Ok(layout) => {
                            let mut x = 0.0;
                            let mut y = 0.0;
                            let mut hit_metrics = DWRITE_HIT_TEST_METRICS::default();
                            let _ = layout.HitTestTextPosition(
                                q_slice.len() as u32,
                                false,
                                &mut x,
                                &mut y,
                                &mut hit_metrics,
                            );
                            48.0 + x
                        }
                        Err(_) => 48.0 + q_slice.len() as f32 * 10.5,
                    }
                };
                target.DrawLine(
                    D2D_POINT_2F {
                        x: caret_x,
                        y: 16.0,
                    },
                    D2D_POINT_2F {
                        x: caret_x,
                        y: 40.0,
                    },
                    &ctx.brushes.accent,
                    2.0,
                    None,
                );
            }

            // 分割线
            if !items.is_empty() {
                let divider_y = metrics::HEADER_HEIGHT as f32;
                target.DrawLine(
                    D2D_POINT_2F {
                        x: 0.0,
                        y: divider_y,
                    },
                    D2D_POINT_2F {
                        x: size.width as f32,
                        y: divider_y,
                    },
                    &ctx.brushes.divider,
                    1.0,
                    None,
                );
            }

            let start_y = metrics::LIST_TOP;
            let item_h = metrics::ITEM_HEIGHT as f32;

            // 绘制弹簧选中高亮胶囊
            if !items.is_empty() {
                let pill_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 8.0,
                        top: pill_y,
                        right: size.width as f32 - 8.0,
                        bottom: pill_y + item_h - 4.0,
                    },
                    radiusX: 8.0,
                    radiusY: 8.0,
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
                    radiusX: 8.0,
                    radiusY: 8.0,
                };

                // Hover 填充
                if Some(i) == hovered && i != selected {
                    target.FillRoundedRectangle(&item_rect, &ctx.brushes.hover);
                }

                let is_calc = matches!(item.kind, ItemKind::Calculator { .. });
                let is_cmd = matches!(item.kind, ItemKind::Command { .. });

                let icon_container = D2D_RECT_F {
                    left: 20.0,
                    top: top + 9.0,
                    right: 52.0,
                    bottom: top + 41.0,
                };
                let badge_fmt = &ctx.formats.badge;

                match &item.kind {
                    ItemKind::Calculator { .. } => {
                        let calc_badge = D2D1_ROUNDED_RECT {
                            rect: icon_container,
                            radiusX: 7.0,
                            radiusY: 7.0,
                        };
                        target.FillRoundedRectangle(&calc_badge, &ctx.brushes.accent_subtle);
                        target.DrawRoundedRectangle(
                            &calc_badge,
                            &ctx.brushes.accent_border,
                            1.0,
                            None,
                        );
                        target.DrawText(
                            BADGE_FX.as_wide(),
                            badge_fmt,
                            &icon_container,
                            &ctx.brushes.accent,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    ItemKind::Command { .. } => {
                        let cmd_badge = D2D1_ROUNDED_RECT {
                            rect: icon_container,
                            radiusX: 7.0,
                            radiusY: 7.0,
                        };
                        target.FillRoundedRectangle(&cmd_badge, &ctx.brushes.badge_bg);
                        target.DrawRoundedRectangle(&cmd_badge, &ctx.brushes.border, 1.0, None);
                        target.DrawText(
                            BADGE_CMD.as_wide(),
                            badge_fmt,
                            &icon_container,
                            &ctx.brushes.admin_badge,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    ItemKind::Application => {
                        let icon_bmp = icon_cache.get_or_load(&target, &item.path);
                        if let Some(bmp) = icon_bmp {
                            target.DrawBitmap(
                                &bmp,
                                Some(&icon_container),
                                1.0,
                                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                                None,
                            );
                        } else {
                            let app_badge = D2D1_ROUNDED_RECT {
                                rect: icon_container,
                                radiusX: 7.0,
                                radiusY: 7.0,
                            };
                            target.FillRoundedRectangle(&app_badge, &ctx.brushes.badge_bg);
                            target.DrawRoundedRectangle(&app_badge, &ctx.brushes.border, 1.0, None);
                            target.DrawText(
                                BADGE_APP.as_wide(),
                                badge_fmt,
                                &icon_container,
                                &ctx.brushes.subtext,
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                    }
                }

                // 标题
                let title_w = to_wide_slice(&item.name);
                let title_brush = if is_calc {
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
                        right: size.width as f32 - 190.0,
                        bottom: top + 27.0,
                    },
                    title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                // 副标题路径
                let sub_w = to_wide_slice(&item.path);
                target.DrawText(
                    &sub_w,
                    &ctx.formats.item_sub,
                    &D2D_RECT_F {
                        left: 62.0,
                        top: top + 27.0,
                        right: size.width as f32 - 190.0,
                        bottom: top + 46.0,
                    },
                    &ctx.brushes.subtext,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                // 按键标识
                if i == selected {
                    let action_str = if is_calc {
                        KEY_CAP_COPY
                    } else if is_cmd {
                        KEY_CAP_RUN
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
                        radiusX: 5.0,
                        radiusY: 5.0,
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

                    if !is_calc {
                        let admin_w = KEY_CAP_ADMIN.as_wide();
                        let admin_rect = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: size.width as f32 - 176.0,
                                top: top + 13.5,
                                right: size.width as f32 - 82.0,
                                bottom: top + 36.5,
                            },
                            radiusX: 5.0,
                            radiusY: 5.0,
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

            let draw_res = target.EndDraw(None, None);
            if let Err(e) = draw_res
                && (e.code().0 as u32) == 0x88982F8C_u32
            {
                self.context = None;
                icon_cache.cache.clear();
            }
        }
        Ok(())
    }
}
