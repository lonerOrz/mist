use crate::config::Config;
use crate::domain::Item;
use crate::history::History;
use crate::renderer::{Renderer, Spring, Theme, metrics, window_scale};
use crate::router;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const TIMER_ANIMATION: usize = 2;

pub struct App {
    pub config: Config,
    pub index: Vec<Item>,
    pub query: String,
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

    pub fn on_query_change(&mut self, hwnd: HWND) {
        self.selected = 0;
        self.hovered = None;
        self.results = router::route_query(&self.query, &self.index, &self.history, &self.config);
        self.pill.reset(metrics::LIST_TOP);
        self.update_window_geometry(hwnd);
    }

    pub fn move_selection_up(&mut self, hwnd: HWND) {
        if self.selected > 0 {
            self.selected -= 1;
            self.pill.set_target(list_item_top(self.selected));
            self.trigger_animation(hwnd);
        }
    }

    pub fn move_selection_down(&mut self, hwnd: HWND) {
        if !self.results.is_empty() && self.selected < self.results.len() - 1 {
            self.selected += 1;
            self.pill.set_target(list_item_top(self.selected));
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
            self.pill.set_target(list_item_top(idx));
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
            self.pill.set_target(list_item_top(idx));
            let item = &self.results[idx];
            let is_calc = matches!(item.kind, crate::domain::ItemKind::Calculator { .. });
            let is_mgmt = matches!(item.kind, crate::domain::ItemKind::AppMgmt);

            let admin_min_x = (self.config.width - metrics::ADMIN_ZONE_FAR) as f32;
            let admin_max_x = (self.config.width - metrics::ADMIN_ZONE_NEAR) as f32;
            let row_top = list_item_top(idx);
            let in_admin_button = y >= row_top + 13.5 && y <= row_top + 36.5;

            if !is_calc && !is_mgmt && in_admin_button && x >= admin_min_x && x <= admin_max_x {
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
            action.execute();
            self.hide(hwnd);
        }
    }

    pub fn execute_selected_admin(&mut self, hwnd: HWND) {
        if let Some(item) = self.results.get(self.selected) {
            let action = item.action.clone();
            let path = item.path.clone();
            self.history.record_launch(&path);
            action.execute_as_admin();
            self.hide(hwnd);
        }
    }

    pub fn hide(&mut self, hwnd: HWND) {
        self.query.clear();
        self.results.clear();
        self.selected = 0;
        self.hovered = None;
        self.height_spring.reset(metrics::HEADER_HEIGHT as f32);
        self.pill.reset(metrics::LIST_TOP);
        self.spring_animating = false;
        unsafe {
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
        unsafe {
            let _ = self.renderer.render(
                hwnd,
                &self.query,
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

fn list_item_top(idx: usize) -> f32 {
    metrics::LIST_TOP + (idx as f32) * metrics::ITEM_HEIGHT as f32
}
