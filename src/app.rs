use crate::clipboard::{ClipboardEntry, ClipboardListener};
use crate::config::{Config, HOTKEY_ID};
use crate::domain::Item;
use crate::history::History;
use crate::query;
use crate::renderer::{Renderer, Spring, Theme, metrics, window_scale};
use std::collections::VecDeque;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::Input::Ime::{
    CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const TIMER_ANIMATION: usize = 2;

/// Tracks which button zone the cursor is currently inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Action,
    Admin,
}

/// Central application state manager coordinating input, animations, and indexing.
pub struct App {
    pub config: Config,
    pub index: Vec<Item>,
    pub query: String,
    pub ime_comp: String,
    pub results: Vec<Item>,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub hovered_btn: Option<ButtonKind>,
    pub caret_visible: bool,
    pub pill: Spring,
    pub height_spring: Spring,
    pub spring_animating: bool,
    pub history: History,
    pub renderer: Renderer,
    pub clipboard_history: VecDeque<ClipboardEntry>,
    pub clipboard_listener: ClipboardListener,
    pub scroll_spring: Spring,
}

impl App {
    /// Creates and initializes the launcher application state.
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
            hovered_btn: None,
            caret_visible: true,
            pill: Spring::new(metrics::LIST_TOP),
            height_spring: Spring::new_slow(metrics::HEADER_HEIGHT as f32, 0.22),
            spring_animating: false,
            history: History::load(),
            renderer,
            clipboard_history: VecDeque::with_capacity(500),
            clipboard_listener: ClipboardListener::new(),
            scroll_spring: Spring::new(0.0),
        }
    }

    /// Replaces the global application index with newly scanned items.
    pub fn set_index(&mut self, items: Vec<Item>) {
        self.index = items;
    }

    /// Applies newly loaded configuration, updating theme, dimensions, and hotkeys.
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

    /// Formats the query display text including in-progress IME composition strings.
    pub fn display_query(&self) -> String {
        if self.ime_comp.is_empty() {
            self.query.clone()
        } else {
            format!("{}{}", self.query, self.ime_comp)
        }
    }

    /// Positions the Windows IME composition candidate window directly beneath the caret.
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

    /// Handles query string changes, re-routes results, and re-computes target window height.
    pub fn on_query_change(&mut self, hwnd: HWND) {
        self.pull_clipboard_updates();
        self.selected = 0;
        self.hovered = None;
        self.hovered_btn = None;
        self.scroll_spring.reset(0.0);
        let cb_slice = self.clipboard_history.make_contiguous();
        self.results = query::route_query(
            &self.query,
            &self.index,
            &self.history,
            &self.config,
            cb_slice,
        );
        self.pill.reset(metrics::LIST_TOP);
        self.update_window_geometry(hwnd);
        self.update_ime_position(hwnd);
    }

    /// Drains new clipboard entries from the background listener thread.
    pub fn pull_clipboard_updates(&mut self) {
        while let Ok(entry) = self.clipboard_listener.rx.try_recv() {
            self.push_unique_front(entry);
        }
    }

    /// Inserts a clipboard entry at the front, deduplicating identical entries and bounding capacity.
    fn push_unique_front(&mut self, entry: ClipboardEntry) {
        let hash = crate::clipboard::calculate_entry_hash(&entry);
        self.clipboard_history
            .retain(|e| crate::clipboard::calculate_entry_hash(e) != hash);
        self.clipboard_history.push_front(entry);
        self.clipboard_history.truncate(500);
    }

    /// Moves the selection cursor up with wrap-around.
    pub fn move_selection_up(&mut self, hwnd: HWND) {
        if self.results.is_empty() {
            return;
        }
        self.selected = if self.selected > 0 {
            self.selected - 1
        } else {
            self.results.len() - 1
        };
        self.pill.set_target(metrics::list_item_top(self.selected));
        self.adjust_scroll_for_selected();
        self.trigger_animation(hwnd);
    }

    /// Moves the selection cursor down with wrap-around.
    pub fn move_selection_down(&mut self, hwnd: HWND) {
        if self.results.is_empty() {
            return;
        }
        self.selected = if self.selected + 1 < self.results.len() {
            self.selected + 1
        } else {
            0
        };
        self.pill.set_target(metrics::list_item_top(self.selected));
        self.adjust_scroll_for_selected();
        self.trigger_animation(hwnd);
    }

    /// Adjusts viewport scroll targets to keep the active selection visible.
    fn adjust_scroll_for_selected(&mut self) {
        let item_h = metrics::ITEM_HEIGHT as f32;
        let visible = self.config.max_results;
        if visible == 0 || self.results.is_empty() {
            return;
        }

        let max_scroll = ((self.results.len().saturating_sub(visible)) as f32 * item_h).max(0.0);

        if self.selected == 0 {
            self.scroll_spring.set_target(0.0);
            return;
        }
        if self.selected == self.results.len() - 1 {
            self.scroll_spring.set_target(max_scroll);
            return;
        }

        let cur_target = self.scroll_spring.target;
        let first_idx = (cur_target / item_h).round() as usize;
        let last_idx = first_idx + visible - 1;

        if self.selected < first_idx {
            self.scroll_spring
                .set_target(((self.selected as f32) * item_h).clamp(0.0, max_scroll));
        } else if self.selected > last_idx {
            self.scroll_spring.set_target(
                (((self.selected + 1 - visible) as f32) * item_h).clamp(0.0, max_scroll),
            );
        }
    }

    /// Handles mouse movement events and updates hover highlighting.
    pub fn on_mouse_move(&mut self, hwnd: HWND, x: f32, y: f32) {
        if self.results.is_empty() || y < metrics::LIST_TOP {
            if self.hovered.take().is_some() || self.hovered_btn.is_some() {
                self.hovered_btn = None;
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            return;
        }
        let item_h = metrics::ITEM_HEIGHT as f32;
        let scroll = self.scroll_spring.current;
        let idx = ((y - metrics::LIST_TOP + scroll) / item_h) as usize;
        if idx < self.results.len() {
            if self.hovered != Some(idx) {
                self.hovered = Some(idx);
                self.trigger_animation(hwnd);
            }
            let item = &self.results[idx];
            let visual_top = metrics::list_item_top(idx) - scroll;
            let in_y = y >= visual_top + 13.5 && y <= visual_top + 36.5;
            if in_y {
                let admin_min_x = self.config.width as f32 - metrics::ADMIN_ZONE_FAR as f32;
                let admin_max_x = self.config.width as f32 - metrics::ADMIN_ZONE_NEAR as f32;
                let action_min_x = self.config.width as f32 - 76.0;
                let action_max_x = self.config.width as f32 - 16.0;
                if item.supports_admin() && x >= admin_min_x && x <= admin_max_x {
                    if self.hovered_btn != Some(ButtonKind::Admin) {
                        self.hovered_btn = Some(ButtonKind::Admin);
                        self.trigger_animation(hwnd);
                    }
                } else if x >= action_min_x && x <= action_max_x {
                    if self.hovered_btn != Some(ButtonKind::Action) {
                        self.hovered_btn = Some(ButtonKind::Action);
                        self.trigger_animation(hwnd);
                    }
                } else if self.hovered_btn.is_some() {
                    self.hovered_btn = None;
                    self.trigger_animation(hwnd);
                }
            } else if self.hovered_btn.is_some() {
                self.hovered_btn = None;
                self.trigger_animation(hwnd);
            }
        } else {
            self.hovered = None;
            self.hovered_btn = None;
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    /// Handles item click and admin badge click events.
    pub fn on_click(&mut self, hwnd: HWND, x: f32, y: f32) {
        if self.results.is_empty() || y < metrics::LIST_TOP {
            return;
        }
        let idx = ((y - metrics::LIST_TOP + self.scroll_spring.current)
            / metrics::ITEM_HEIGHT as f32) as usize;
        if idx < self.results.len() {
            self.selected = idx;
            self.pill.set_target(metrics::list_item_top(idx));
            let item = &self.results[idx];

            if item.supports_admin()
                && metrics::is_in_admin_button(
                    idx,
                    self.config.width as f32,
                    x,
                    y,
                    self.scroll_spring.current,
                )
            {
                self.execute_selected_admin(hwnd);
            } else {
                self.execute_selected(hwnd);
            }
        }
    }

    /// Executes the active selected item under normal privileges.
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

    /// Executes the active selected item with Administrator elevation.
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

    /// Hides the launcher window, clears input buffers, and returns physical working set memory.
    pub fn hide(&mut self, hwnd: HWND) {
        self.query.clear();
        self.query.shrink_to_fit();
        self.ime_comp.clear();
        self.ime_comp.shrink_to_fit();
        self.results.clear();
        self.results.shrink_to_fit();
        self.selected = 0;
        self.hovered = None;
        self.hovered_btn = None;
        self.height_spring.reset(metrics::HEADER_HEIGHT as f32);
        self.pill.reset(metrics::LIST_TOP);
        self.scroll_spring.reset(0.0);
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

    /// Updates animation physics step and renders the current frame to the Direct2D target.
    pub fn render_current_frame(&mut self, hwnd: HWND) {
        let pill_moving = !self.results.is_empty() && self.pill.update(1.0 / 60.0);
        let height_moving = self.height_spring.update(1.0 / 60.0);
        let scroll_moving = self.scroll_spring.update(1.0 / 60.0);
        self.spring_animating = pill_moving || height_moving || scroll_moving;

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
                self.config.max_results,
                self.scroll_spring.current,
                self.hovered_btn,
            );
        }
    }

    /// Handles mouse wheel scrolling for search result lists.
    pub fn on_mouse_wheel(&mut self, hwnd: HWND, delta: i16) {
        let visible = self.config.max_results;
        if self.results.len() <= visible || visible == 0 {
            return;
        }
        let item_h = metrics::ITEM_HEIGHT as f32;
        let max_scroll = ((self.results.len() - visible) as f32) * item_h;
        let step = item_h * if delta > 0 { -1.0 } else { 1.0 };
        self.scroll_spring
            .set_target((self.scroll_spring.target + step).clamp(0.0, max_scroll));
        self.trigger_animation(hwnd);
    }

    /// Activates the 60 FPS animation timer and triggers a client area repaint.
    pub fn trigger_animation(&mut self, hwnd: HWND) {
        self.spring_animating = true;
        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_ANIMATION, 16, None);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    /// Recalculates the target height spring value based on result count.
    fn update_window_geometry(&mut self, hwnd: HWND) {
        let count = self.results.len().min(self.config.max_results);
        let new_h = if count == 0 {
            metrics::HEADER_HEIGHT
        } else {
            metrics::HEADER_HEIGHT + 8 + (count as i32) * metrics::ITEM_HEIGHT + 6
        };
        self.height_spring.target = new_h as f32;
        self.trigger_animation(hwnd);
    }
}
