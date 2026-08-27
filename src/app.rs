use crate::config::{Config, HOTKEY_ID};
use crate::domain::Item;
use crate::history::History;
use crate::renderer::{Renderer, Spring, Theme, metrics, window_scale};
use crate::router;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::Input::Ime::{
    CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const TIMER_ANIMATION: usize = 2;

pub struct App {
    pub config: Config,
    pub index: Vec<Item>,
    pub query: String,
    pub ime_comp: String,
    pub results: Vec<Item>,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub caret_visible: bool,
    pub pill: Spring,
    pub height_spring: Spring,
    pub spring_animating: bool,
    pub history: History,
    pub renderer: Renderer,
}

impl App {
    pub fn new(renderer: Renderer, config: Config) -> Self {
        let mut renderer = renderer;
        renderer.set_theme(Theme::from_config(&config));
        Self {
            config,
            index: Vec::new(),
            query: String::new(),
            ime_comp: String::new(),
            results: Vec::new(),
            selected: 0,
            hovered: None,
            caret_visible: true,
            pill: Spring::new(metrics::LIST_TOP),
            height_spring: Spring::new_slow(metrics::HEADER_HEIGHT as f32, 0.22),
            spring_animating: false,
            history: History::load(),
            renderer,
        }
    }

    pub fn set_index(&mut self, items: Vec<Item>) {
        self.index = items;
    }

    pub fn update_config(&mut self, hwnd: HWND, new_config: Config) {
        if self.config.font_family != new_config.font_family {
            self.renderer
                .set_font_family(new_config.font_family.clone());
        }
        self.renderer.set_theme(Theme::from_config(&new_config));
        self.config = new_config;
        self.on_query_change(hwnd);
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let s = window_scale(hwnd);
                let h = self.height_spring.current.round() as i32;
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    (self.config.width as f32 * s).round() as i32,
                    (h as f32 * s).round() as i32,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    pub fn display_query(&self) -> String {
        if self.ime_comp.is_empty() {
            self.query.clone()
        } else {
            format!("{}{}", self.query, self.ime_comp)
        }
    }

    pub fn update_ime_position(&mut self, hwnd: HWND) {
        unsafe {
            let himc = ImmGetContext(hwnd);
            if himc.0.is_null() {
                return;
            }

            let s = window_scale(hwnd);
            let input_clip_left = metrics::INPUT_X;
            let input_clip_right = self.config.width as f32 - 24.0;
            let max_visible_width = input_clip_right - input_clip_left;

            let display_text = self.display_query();
            let caret_offset_x = self.renderer.calculate_caret_offset(hwnd, &display_text);
            let scroll_x = if caret_offset_x > max_visible_width {
                caret_offset_x - max_visible_width
            } else {
                0.0
            };

            let final_caret_x = input_clip_left + caret_offset_x - scroll_x;
            let pt_x = (final_caret_x * s).round() as i32;
            let pt_y = ((metrics::HEADER_HEIGHT as f32 - 10.0) * s).round() as i32;

            let form = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: POINT { x: pt_x, y: pt_y },
                rcArea: Default::default(),
            };

            let _ = ImmSetCompositionWindow(himc, &form);
            let _ = ImmReleaseContext(hwnd, himc);
        }
    }

    pub fn on_query_change(&mut self, hwnd: HWND) {
        self.selected = 0;
        self.hovered = None;
        self.results = router::route_query(&self.query, &self.index, &self.history, &self.config);
        self.pill.reset(metrics::LIST_TOP);
        self.update_window_geometry(hwnd);
        self.update_ime_position(hwnd);
    }

    pub fn move_selection_up(&mut self, hwnd: HWND) {
        if self.selected > 0 {
            self.selected -= 1;
            self.pill.set_target(metrics::list_item_top(self.selected));
            self.trigger_animation(hwnd);
        }
    }

    pub fn move_selection_down(&mut self, hwnd: HWND) {
        if !self.results.is_empty() && self.selected < self.results.len() - 1 {
            self.selected += 1;
            self.pill.set_target(metrics::list_item_top(self.selected));
            self.trigger_animation(hwnd);
        }
    }

    pub fn on_mouse_move(&mut self, hwnd: HWND, y: f32) {
        if self.results.is_empty() || y < metrics::LIST_TOP {
            if self.hovered.take().is_some() {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            return;
        }
        let idx = ((y - metrics::LIST_TOP) / metrics::ITEM_HEIGHT as f32) as usize;
        if idx < self.results.len() {
            let changed = self.hovered != Some(idx) || self.selected != idx;
            self.hovered = Some(idx);
            self.selected = idx;
            self.pill.set_target(metrics::list_item_top(idx));
            if changed {
                self.trigger_animation(hwnd);
            }
        } else if self.hovered.take().is_some() {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    pub fn on_click(&mut self, hwnd: HWND, x: f32, y: f32) {
        if self.results.is_empty() || y < metrics::LIST_TOP {
            return;
        }
        let idx = ((y - metrics::LIST_TOP) / metrics::ITEM_HEIGHT as f32) as usize;
        if idx < self.results.len() {
            self.selected = idx;
            self.pill.set_target(metrics::list_item_top(idx));
            let item = &self.results[idx];
            let is_calc = matches!(item.kind, crate::domain::ItemKind::Calculator { .. });
            let is_mgmt = matches!(item.kind, crate::domain::ItemKind::AppMgmt);

            // 统一调用 metrics 模块判定点击区域，消除硬编码魔法数字
            if !is_calc
                && !is_mgmt
                && metrics::is_in_admin_button(idx, self.config.width as f32, x, y)
            {
                self.execute_selected_admin(hwnd);
            } else {
                self.execute_selected(hwnd);
            }
        }
    }

    pub fn execute_selected(&mut self, hwnd: HWND) {
        if let Some(item) = self.results.get(self.selected) {
            let action = item.action.clone();
            let path = item.path.clone();
            self.history.record_launch(&path);

            if matches!(action, crate::domain::Action::RestartApp) {
                unsafe {
                    let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                }
                action.execute();
                return;
            }

            action.execute();
            self.hide(hwnd);
        }
    }

    pub fn execute_selected_admin(&mut self, hwnd: HWND) {
        if let Some(item) = self.results.get(self.selected) {
            let action = item.action.clone();
            let path = item.path.clone();
            self.history.record_launch(&path);

            if matches!(action, crate::domain::Action::RestartApp) {
                unsafe {
                    let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                }
                action.execute();
                return;
            }

            action.execute_as_admin();
            self.hide(hwnd);
        }
    }

    pub fn hide(&mut self, hwnd: HWND) {
        self.query.clear();
        self.query.shrink_to_fit();
        self.ime_comp.clear();
        self.ime_comp.shrink_to_fit();
        self.results.clear();
        self.results.shrink_to_fit();
        self.selected = 0;
        self.hovered = None;
        self.height_spring.reset(metrics::HEADER_HEIGHT as f32);
        self.pill.reset(metrics::LIST_TOP);
        self.spring_animating = false;
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ANIMATION);
            let _ = KillTimer(Some(hwnd), crate::TIMER_CARET);

            let s = window_scale(hwnd);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                (self.config.width as f32 * s).round() as i32,
                (metrics::HEADER_HEIGHT as f32 * s).round() as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }

    pub fn render_current_frame(&mut self, hwnd: HWND) {
        let pill_moving = !self.results.is_empty() && self.pill.update(1.0 / 60.0);
        let height_moving = self.height_spring.update(1.0 / 60.0);
        self.spring_animating = pill_moving || height_moving;
        if height_moving {
            let h = self.height_spring.current.round() as i32;
            let s = window_scale(hwnd);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    (self.config.width as f32 * s).round() as i32,
                    (h as f32 * s).round() as i32,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
            }
        }
        let items: Vec<&Item> = self.results.iter().collect();
        let pill_y = if self.results.is_empty() {
            metrics::LIST_TOP
        } else {
            self.pill.current
        };
        let display_text = self.display_query();
        unsafe {
            let _ = self.renderer.render(
                hwnd,
                &display_text,
                &self.config.placeholder,
                &items,
                self.selected,
                self.caret_visible,
                self.hovered,
                pill_y,
            );
        }
    }

    pub fn trigger_animation(&mut self, hwnd: HWND) {
        self.spring_animating = true;
        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_ANIMATION, 16, None);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn update_window_geometry(&mut self, hwnd: HWND) {
        let count = self.results.len();
        let new_h = if count == 0 {
            metrics::HEADER_HEIGHT
        } else {
            metrics::HEADER_HEIGHT + 8 + (count as i32) * metrics::ITEM_HEIGHT + 6
        };
        self.height_spring.target = new_h as f32;
        self.trigger_animation(hwnd);
    }
}
