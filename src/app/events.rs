use super::*;
use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct WheelTuning {
    queue_limit: isize,
    medium_threshold: u8,
    fast_threshold: u8,
    medium_divisor: isize,
    fast_divisor: isize,
}

const ENTRY_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: WHEEL_SCROLL_QUEUE_LIMIT,
    medium_threshold: 3,
    fast_threshold: 6,
    medium_divisor: 2,
    fast_divisor: 4,
};
const ENTRY_HORIZONTAL_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: WHEEL_SCROLL_QUEUE_LIMIT_HORIZONTAL,
    medium_threshold: 2,
    fast_threshold: 4,
    medium_divisor: 2,
    fast_divisor: 3,
};
const PREVIEW_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: WHEEL_SCROLL_QUEUE_LIMIT,
    medium_threshold: 4,
    fast_threshold: 8,
    medium_divisor: 2,
    fast_divisor: 4,
};
const PREVIEW_HORIZONTAL_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: WHEEL_SCROLL_QUEUE_LIMIT_PREVIEW_HORIZONTAL,
    medium_threshold: 2,
    fast_threshold: 4,
    medium_divisor: 2,
    fast_divisor: 3,
};
const SEARCH_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: WHEEL_SCROLL_QUEUE_LIMIT_SEARCH,
    medium_threshold: 2,
    fast_threshold: 4,
    medium_divisor: 2,
    fast_divisor: 3,
};
const HIGH_FREQUENCY_ENTRY_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: 4,
    medium_threshold: 2,
    fast_threshold: 4,
    medium_divisor: 4,
    fast_divisor: 8,
};
const HIGH_FREQUENCY_ENTRY_HORIZONTAL_WHEEL_TUNING: WheelTuning = WheelTuning {
    queue_limit: 2,
    medium_threshold: 2,
    fast_threshold: 3,
    medium_divisor: 3,
    fast_divisor: 5,
};

impl App {
    pub fn handle_event(&mut self, event: Event) -> Result<()> {
        let result = match event {
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => Ok(()),
        };

        if let Err(error) = result {
            self.report_runtime_error("Action failed", &error);
        }

        Ok(())
    }

    pub fn process_pending_scroll(&mut self) -> bool {
        let mut dirty = false;

        if self.search.is_some() {
            self.wheel_scroll.vertical.pending = 0;
            self.wheel_scroll.horizontal.pending = 0;
            self.wheel_scroll.preview.pending = 0;
            self.wheel_scroll.preview_horizontal.pending = 0;
            dirty |= self.flush_search_scroll();
        } else {
            self.wheel_scroll.search.pending = 0;
            dirty |= self.flush_entry_vertical_scroll();
            dirty |= self.flush_preview_scroll();
            dirty |= self.flush_preview_horizontal_scroll();
            if self.view_mode == ViewMode::Grid {
                dirty |= self.flush_entry_horizontal_scroll();
            } else {
                self.wheel_scroll.horizontal.pending = 0;
            }
        }

        dirty
    }

    pub fn has_pending_scroll(&self) -> bool {
        self.wheel_scroll.vertical.pending != 0
            || self.wheel_scroll.horizontal.pending != 0
            || self.wheel_scroll.preview.pending != 0
            || self.wheel_scroll.preview_horizontal.pending != 0
            || self.wheel_scroll.search.pending != 0
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.search.is_some() {
            return self.handle_search_key(key);
        }

        if self.help_open {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c'))
            {
                self.help_open = false;
                return Ok(());
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help_open = false,
                _ => {}
            }
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('f') => {
                    if let Err(error) = self.open_fuzzy_finder(SearchScope::Files) {
                        self.status = format!("Search unavailable: {error}");
                    }
                    return Ok(());
                }
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.adjust_zoom(1);
                    return Ok(());
                }
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    self.adjust_zoom(-1);
                    return Ok(());
                }
                KeyCode::Char('0') => {
                    self.reset_zoom();
                    return Ok(());
                }
                _ => {}
            }
        }

        if self.wheel_profile == WheelProfile::HighFrequency
            && key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
        {
            match key.code {
                KeyCode::Left => {
                    if self.handle_horizontal_navigation_key(-1) {
                        return Ok(());
                    }
                }
                KeyCode::Right => {
                    if self.handle_horizontal_navigation_key(1) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Left => return self.go_back(),
                KeyCode::Right => return self.go_forward(),
                _ => {}
            }
        }

        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Char('[') => {
                    if self.step_pdf_page(-1) {
                        return Ok(());
                    }
                }
                KeyCode::Char(']') => {
                    if self.step_pdf_page(1) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => {
                self.clear_wheel_scroll();
                self.help_open = true;
            }
            KeyCode::Tab => self.step_pinned_place(1)?,
            KeyCode::BackTab => self.step_pinned_place(-1)?,
            KeyCode::Up | KeyCode::Char('k') => self.move_vertical(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_vertical(1),
            KeyCode::Left | KeyCode::Char('h') => {
                if self.view_mode == ViewMode::Grid {
                    self.move_by(-1);
                } else {
                    self.go_parent()?;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.view_mode == ViewMode::Grid {
                    self.move_by(1);
                } else if self.selected_entry().is_some_and(Entry::is_dir) {
                    self.open_selected()?;
                } else {
                    self.status = "Press Enter to open files".to_string();
                }
            }
            KeyCode::PageUp => self.page(-1),
            KeyCode::PageDown => self.page(1),
            KeyCode::Home => self.select_index(0),
            KeyCode::End => self.select_last(),
            KeyCode::Char('g') => self.select_index(0),
            KeyCode::Char('G') => self.select_last(),
            KeyCode::Enter => self.open_selected()?,
            KeyCode::Backspace => self.go_parent()?,
            KeyCode::Char('v') => {
                self.clear_wheel_scroll();
                self.view_mode = self.view_mode.toggle();
                self.sync_scroll();
                self.status = format!("Switched to {} view", self.view_mode.label());
            }
            KeyCode::Char('s') => {
                self.sort_mode = self.sort_mode.cycle();
                self.reload()?;
                self.status = format!("Sort: {}", self.sort_mode.label());
            }
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.reload()?;
                self.status = if self.show_hidden {
                    "Hidden files shown".to_string()
                } else {
                    "Hidden files hidden".to_string()
                };
            }
            KeyCode::Char('f') => {
                if let Err(error) = self.open_fuzzy_finder(SearchScope::Folders) {
                    self.status = format!("Search unavailable: {error}");
                }
            }
            KeyCode::Char('o') => self.open_in_system()?,
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        if self.search.is_some() {
            return self.handle_search_mouse(mouse);
        }

        if self.help_open {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.clear_wheel_scroll();
                self.help_open = false;
            }
            return Ok(());
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.update_wheel_target_from_position(mouse.column, mouse.row);
                if let Some(rect) = self.frame_state.back_button
                    && rect_contains(rect, mouse.column, mouse.row)
                {
                    return self.go_back();
                }
                if let Some(rect) = self.frame_state.forward_button
                    && rect_contains(rect, mouse.column, mouse.row)
                {
                    return self.go_forward();
                }
                if let Some(rect) = self.frame_state.parent_button
                    && rect_contains(rect, mouse.column, mouse.row)
                {
                    return self.go_parent();
                }
                if let Some(rect) = self.frame_state.hidden_button
                    && rect_contains(rect, mouse.column, mouse.row)
                {
                    self.show_hidden = !self.show_hidden;
                    self.reload()?;
                    self.status = if self.show_hidden {
                        "Hidden files shown".to_string()
                    } else {
                        "Hidden files hidden".to_string()
                    };
                    return Ok(());
                }
                if let Some(rect) = self.frame_state.view_button
                    && rect_contains(rect, mouse.column, mouse.row)
                {
                    self.clear_wheel_scroll();
                    self.view_mode = self.view_mode.toggle();
                    self.sync_scroll();
                    self.status = format!("Switched to {} view", self.view_mode.label());
                    return Ok(());
                }

                if let Some(target) = self
                    .frame_state
                    .sidebar_hits
                    .iter()
                    .find(|hit| rect_contains(hit.rect, mouse.column, mouse.row))
                    .cloned()
                {
                    return self.set_dir(target.path);
                }

                if let Some(hit) = self
                    .frame_state
                    .entry_hits
                    .iter()
                    .find(|hit| rect_contains(hit.rect, mouse.column, mouse.row))
                    .cloned()
                {
                    let Some(path) = self.entries.get(hit.index).map(|entry| entry.path.clone())
                    else {
                        return Ok(());
                    };
                    self.select_index(hit.index);
                    if self.is_double_click(&path) {
                        self.open_selected()?;
                    }
                    self.last_click = Some(ClickState {
                        path,
                        at: Instant::now(),
                    });
                }
            }
            MouseEventKind::ScrollDown => {
                self.handle_wheel_event(mouse, 1);
            }
            MouseEventKind::ScrollUp => {
                self.handle_wheel_event(mouse, -1);
            }
            MouseEventKind::ScrollLeft => {
                self.handle_horizontal_wheel_event(mouse, -1);
            }
            MouseEventKind::ScrollRight => {
                self.handle_horizontal_wheel_event(mouse, 1);
            }
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.update_wheel_target_from_position(mouse.column, mouse.row);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_wheel_event(&mut self, mouse: MouseEvent, delta: isize) {
        let target = self
            .high_frequency_preview_target(false)
            .or_else(|| self.resolve_wheel_target(mouse.column, mouse.row));
        match target {
            Some(WheelTarget::Preview) => {
                if self.pdf_page_wheel_navigation_active() && self.step_pdf_page(delta) {
                    return;
                }
                self.focus_preview_scroll();
                if mouse.modifiers.contains(KeyModifiers::SHIFT)
                    && self.preview_allows_horizontal_scroll()
                {
                    Self::queue_scroll(
                        &mut self.wheel_scroll.preview_horizontal,
                        delta,
                        PREVIEW_HORIZONTAL_WHEEL_TUNING,
                    );
                } else {
                    Self::queue_scroll(&mut self.wheel_scroll.preview, delta, PREVIEW_WHEEL_TUNING);
                }
            }
            Some(WheelTarget::Entries) | None => {
                self.focus_entry_scroll();
                if self.view_mode == ViewMode::Grid && mouse.modifiers.contains(KeyModifiers::SHIFT)
                {
                    let tuning = self.entry_horizontal_wheel_tuning();
                    Self::queue_scroll(&mut self.wheel_scroll.horizontal, delta, tuning);
                } else {
                    let tuning = self.entry_wheel_tuning();
                    Self::queue_scroll(&mut self.wheel_scroll.vertical, delta, tuning);
                }
            }
        }
    }

    fn handle_horizontal_wheel_event(&mut self, mouse: MouseEvent, delta: isize) {
        let target = self
            .high_frequency_preview_target(true)
            .or_else(|| self.resolve_wheel_target(mouse.column, mouse.row));
        match target {
            Some(WheelTarget::Preview) => {
                self.focus_preview_scroll();
                if self.preview_allows_horizontal_scroll() {
                    Self::queue_scroll(
                        &mut self.wheel_scroll.preview_horizontal,
                        delta,
                        PREVIEW_HORIZONTAL_WHEEL_TUNING,
                    );
                }
            }
            Some(WheelTarget::Entries) | None => {
                if self.view_mode != ViewMode::Grid {
                    return;
                }
                self.focus_entry_scroll();
                let tuning = self.entry_horizontal_wheel_tuning();
                Self::queue_scroll(&mut self.wheel_scroll.horizontal, delta, tuning);
            }
        }
    }

    pub(super) fn queue_search_wheel(&mut self, delta: isize) {
        Self::queue_scroll(&mut self.wheel_scroll.search, delta, SEARCH_WHEEL_TUNING);
    }

    pub fn open_selected(&mut self) -> Result<()> {
        let Some(entry) = self.selected_entry() else {
            return Ok(());
        };
        if entry.is_dir() {
            self.set_dir(entry.path.clone())
        } else {
            self.open_in_system()
        }
    }

    fn queue_scroll(lane: &mut ScrollLane, delta: isize, tuning: WheelTuning) {
        let now = Instant::now();
        let direction = delta.signum();
        let continuing_burst = lane.last_input_direction == direction
            && lane
                .last_input_at
                .is_some_and(|at| now.duration_since(at) <= WHEEL_SCROLL_BURST_WINDOW);

        if continuing_burst {
            lane.burst_count = lane.burst_count.saturating_add(1);
        } else {
            lane.remainder = 0;
            lane.burst_count = 1;
        }
        lane.last_input_at = Some(now);
        lane.last_input_direction = direction;

        let divisor = if lane.burst_count >= tuning.fast_threshold {
            tuning.fast_divisor
        } else if lane.burst_count >= tuning.medium_threshold {
            tuning.medium_divisor
        } else {
            1
        };

        if divisor <= 1 {
            lane.pending = (lane.pending + delta).clamp(-tuning.queue_limit, tuning.queue_limit);
            return;
        }

        lane.remainder += delta;
        while lane.remainder.abs() >= divisor {
            let step = lane.remainder.signum();
            lane.pending = (lane.pending + step).clamp(-tuning.queue_limit, tuning.queue_limit);
            lane.remainder -= step * divisor;
        }
    }

    fn consume_scroll_step(lane: &mut ScrollLane, cooldown: Duration) -> Option<isize> {
        let now = Instant::now();
        if lane.pending == 0 {
            return None;
        }
        if lane
            .last_step_at
            .is_some_and(|at| now.duration_since(at) < cooldown)
        {
            return None;
        }

        let step = lane.pending.signum();
        lane.pending -= step;
        lane.last_step_at = Some(now);
        Some(step)
    }

    fn focus_preview_scroll(&mut self) {
        self.last_wheel_target = Some(WheelTarget::Preview);
        Self::reset_scroll_lane(&mut self.wheel_scroll.vertical);
        Self::reset_scroll_lane(&mut self.wheel_scroll.horizontal);
    }

    fn focus_entry_scroll(&mut self) {
        self.last_wheel_target = Some(WheelTarget::Entries);
        Self::reset_scroll_lane(&mut self.wheel_scroll.preview);
        Self::reset_scroll_lane(&mut self.wheel_scroll.preview_horizontal);
    }

    fn reset_scroll_lane(lane: &mut ScrollLane) {
        lane.pending = 0;
        lane.remainder = 0;
        lane.last_step_at = None;
        lane.last_input_at = None;
        lane.last_input_direction = 0;
        lane.burst_count = 0;
    }

    fn flush_entry_vertical_scroll(&mut self) -> bool {
        let interval = self.entry_scroll_interval();
        let Some(step) = Self::consume_scroll_step(&mut self.wheel_scroll.vertical, interval)
        else {
            return false;
        };

        let previous = self.selected;
        self.move_vertical(step);
        previous != self.selected
    }

    fn flush_entry_horizontal_scroll(&mut self) -> bool {
        let Some(step) = Self::consume_scroll_step(
            &mut self.wheel_scroll.horizontal,
            WHEEL_SCROLL_INTERVAL_HORIZONTAL,
        ) else {
            return false;
        };

        let previous = self.selected;
        self.move_by(step);
        previous != self.selected
    }

    fn flush_search_scroll(&mut self) -> bool {
        let Some(step) =
            Self::consume_scroll_step(&mut self.wheel_scroll.search, WHEEL_SCROLL_INTERVAL_SEARCH)
        else {
            return false;
        };

        let previous = self
            .search
            .as_ref()
            .map(|search| search.selected)
            .unwrap_or(0);
        self.move_search_selection(step);
        self.search
            .as_ref()
            .map(|search| search.selected != previous)
            .unwrap_or(false)
    }

    fn flush_preview_scroll(&mut self) -> bool {
        let mut dirty = false;
        for _ in 0..2 {
            let Some(step) = Self::consume_scroll_step(
                &mut self.wheel_scroll.preview,
                WHEEL_SCROLL_INTERVAL_PREVIEW,
            ) else {
                break;
            };
            dirty |= self.scroll_preview_lines(step);
        }
        dirty
    }

    fn flush_preview_horizontal_scroll(&mut self) -> bool {
        let mut dirty = false;
        for _ in 0..2 {
            let Some(step) = Self::consume_scroll_step(
                &mut self.wheel_scroll.preview_horizontal,
                WHEEL_SCROLL_INTERVAL_PREVIEW_HORIZONTAL,
            ) else {
                break;
            };
            dirty |= self.scroll_preview_columns(step);
        }
        dirty
    }

    fn preview_scroll_step(&self) -> usize {
        self.frame_state
            .preview_rows_visible
            .saturating_div(6)
            .clamp(2, 4)
    }

    fn preview_horizontal_scroll_step(&self) -> usize {
        self.frame_state
            .preview_cols_visible
            .saturating_div(20)
            .clamp(1, 3)
    }

    pub(super) fn sync_preview_scroll(&mut self) -> bool {
        let previous = self.preview_scroll;
        let previous_horizontal = self.preview_horizontal_scroll;
        let visible_rows = self.frame_state.preview_rows_visible;
        let visible_cols = self.frame_state.preview_cols_visible;
        let max_scroll = self
            .preview_total_lines(visible_cols)
            .saturating_sub(visible_rows.max(1));
        self.preview_scroll = self.preview_scroll.min(max_scroll);
        let max_horizontal = self.preview_max_horizontal_scroll(visible_cols);
        self.preview_horizontal_scroll = self.preview_horizontal_scroll.min(max_horizontal);
        previous != self.preview_scroll || previous_horizontal != self.preview_horizontal_scroll
    }

    pub(super) fn clear_wheel_scroll(&mut self) {
        Self::reset_scroll_lane(&mut self.wheel_scroll.vertical);
        Self::reset_scroll_lane(&mut self.wheel_scroll.horizontal);
        Self::reset_scroll_lane(&mut self.wheel_scroll.preview);
        Self::reset_scroll_lane(&mut self.wheel_scroll.preview_horizontal);
        Self::reset_scroll_lane(&mut self.wheel_scroll.search);
    }

    fn entry_wheel_tuning(&self) -> WheelTuning {
        match self.wheel_profile {
            WheelProfile::Default => ENTRY_WHEEL_TUNING,
            WheelProfile::HighFrequency => HIGH_FREQUENCY_ENTRY_WHEEL_TUNING,
        }
    }

    fn entry_horizontal_wheel_tuning(&self) -> WheelTuning {
        match self.wheel_profile {
            WheelProfile::Default => ENTRY_HORIZONTAL_WHEEL_TUNING,
            WheelProfile::HighFrequency => HIGH_FREQUENCY_ENTRY_HORIZONTAL_WHEEL_TUNING,
        }
    }

    fn entry_scroll_interval(&self) -> Duration {
        match self.wheel_profile {
            WheelProfile::Default => WHEEL_SCROLL_INTERVAL_VERTICAL,
            WheelProfile::HighFrequency => WHEEL_SCROLL_INTERVAL_VERTICAL_HIGH_FREQUENCY,
        }
    }

    fn handle_horizontal_navigation_key(&mut self, delta: isize) -> bool {
        if self.last_wheel_target == Some(WheelTarget::Preview) {
            if self.wheel_profile == WheelProfile::HighFrequency {
                let _ = self.scroll_preview_columns(delta);
                return true;
            }
            if self.preview_allows_horizontal_scroll()
                && self.preview_max_horizontal_scroll(self.frame_state.preview_cols_visible.max(1))
                    > 0
            {
                return self.scroll_preview_columns(delta);
            }
            self.last_wheel_target = Some(WheelTarget::Entries);
        }

        if self.wheel_profile == WheelProfile::HighFrequency
            && self.high_frequency_preview_target(true) == Some(WheelTarget::Preview)
            && self.preview_allows_horizontal_scroll()
        {
            self.last_wheel_target = Some(WheelTarget::Preview);
            let _ = self.scroll_preview_columns(delta);
            return true;
        }

        if self.wheel_profile == WheelProfile::HighFrequency && self.view_mode == ViewMode::Grid {
            self.last_wheel_target = Some(WheelTarget::Entries);
            self.focus_entry_scroll();
            let tuning = self.entry_horizontal_wheel_tuning();
            Self::queue_scroll(&mut self.wheel_scroll.horizontal, delta, tuning);
            return true;
        }

        false
    }

    fn pdf_page_wheel_navigation_active(&self) -> bool {
        self.preview_uses_image_overlay() || self.preview_prefers_pdf_surface()
    }

    fn preview_has_vertical_overflow(&self) -> bool {
        let visible_cols = self.frame_state.preview_cols_visible.max(1);
        let visible_rows = self.frame_state.preview_rows_visible.max(1);
        self.preview_total_lines(visible_cols) > visible_rows
    }

    fn preview_auto_focus_ready(&self) -> bool {
        self.preview_has_vertical_overflow()
            && self.last_selection_change_at.elapsed() >= PREVIEW_AUTO_FOCUS_DELAY
    }

    fn preview_horizontal_auto_focus_ready(&self) -> bool {
        self.preview_allows_horizontal_scroll()
            && self.preview_max_horizontal_scroll(self.frame_state.preview_cols_visible.max(1)) > 0
            && self.last_selection_change_at.elapsed() >= PREVIEW_AUTO_FOCUS_DELAY
    }

    fn high_frequency_preview_target(&self, horizontal: bool) -> Option<WheelTarget> {
        if self.wheel_profile != WheelProfile::HighFrequency {
            return None;
        }

        if self.last_wheel_target == Some(WheelTarget::Preview) {
            return Some(WheelTarget::Preview);
        }

        let preview_ready = if horizontal {
            self.preview_horizontal_auto_focus_ready()
        } else {
            self.preview_auto_focus_ready()
        };

        preview_ready.then_some(WheelTarget::Preview)
    }

    fn scroll_preview_lines(&mut self, delta: isize) -> bool {
        let previous = self.preview_scroll;
        let step = self.preview_scroll_step();
        if delta.is_negative() {
            self.preview_scroll = self
                .preview_scroll
                .saturating_sub(step.saturating_mul(delta.unsigned_abs()));
        } else {
            self.preview_scroll = self
                .preview_scroll
                .saturating_add(step.saturating_mul(delta as usize));
        }
        self.sync_preview_scroll();
        previous != self.preview_scroll
    }

    fn scroll_preview_columns(&mut self, delta: isize) -> bool {
        let previous = self.preview_horizontal_scroll;
        let step = self.preview_horizontal_scroll_step();
        if delta.is_negative() {
            self.preview_horizontal_scroll = self
                .preview_horizontal_scroll
                .saturating_sub(step.saturating_mul(delta.unsigned_abs()));
        } else {
            self.preview_horizontal_scroll = self
                .preview_horizontal_scroll
                .saturating_add(step.saturating_mul(delta as usize));
        }
        self.sync_preview_scroll();
        previous != self.preview_horizontal_scroll
    }

    fn panel_target_at(&self, column: u16, row: u16) -> Option<WheelTarget> {
        if self
            .frame_state
            .preview_panel
            .is_some_and(|rect| rect_contains(rect, column, row))
        {
            Some(WheelTarget::Preview)
        } else if self
            .frame_state
            .entries_panel
            .is_some_and(|rect| rect_contains(rect, column, row))
        {
            Some(WheelTarget::Entries)
        } else {
            None
        }
    }

    fn update_wheel_target_from_position(&mut self, column: u16, row: u16) {
        if let Some(target) = self.panel_target_at(column, row) {
            self.last_wheel_target = Some(target);
        }
    }

    fn resolve_wheel_target(&mut self, column: u16, row: u16) -> Option<WheelTarget> {
        if let Some(target) = self.panel_target_at(column, row) {
            self.last_wheel_target = Some(target);
            return Some(target);
        }

        if let Some(preview) = self.frame_state.preview_panel
            && column >= preview.x
        {
            self.last_wheel_target = Some(WheelTarget::Preview);
            return self.last_wheel_target;
        }

        if let Some(entries) = self.frame_state.entries_panel
            && column >= entries.x
            && column < entries.x.saturating_add(entries.width)
        {
            self.last_wheel_target = Some(WheelTarget::Entries);
            return self.last_wheel_target;
        }

        self.last_wheel_target
    }

    fn is_double_click(&self, path: &Path) -> bool {
        self.last_click
            .as_ref()
            .is_some_and(|click| click.path == path && click.at.elapsed() <= DOUBLE_CLICK_WINDOW)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::{
        env, fs,
        path::PathBuf,
        thread,
        time::Duration,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        env::temp_dir().join(format!("elio-events-{label}-{unique}"))
    }

    fn wait_for_directory_load(app: &mut App) {
        for _ in 0..100 {
            let _ = app.process_background_jobs();
            if app.pending_directory_load.is_none() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for directory load");
    }

    #[test]
    fn right_arrow_does_not_open_selected_file_in_list_view() {
        let root = temp_path("right-file");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let file_path = root.join("note.txt");
        fs::write(&file_path, "hello").expect("failed to write temp file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);

        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )))
        .expect("right arrow should be handled");

        assert_eq!(app.cwd, root);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(file_path.as_path())
        );
        assert_eq!(app.status_message(), "Press Enter to open files");

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn right_arrow_enters_selected_directory_in_list_view() {
        let root = temp_path("right-dir");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create temp dirs");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);

        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )))
        .expect("right arrow should be handled");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, child);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn left_arrow_in_list_view_reselects_previous_directory_in_parent() {
        let root = temp_path("left-parent-selection");
        let alpha = root.join("alpha");
        let child = root.join("child");
        fs::create_dir_all(&alpha).expect("failed to create alpha dir");
        fs::create_dir_all(&child).expect("failed to create child dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(1);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);

        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)))
            .expect("left arrow should be handled");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, root);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(child.as_path())
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn go_back_reselects_previous_directory_in_parent() {
        let root = temp_path("history-back-selection");
        let alpha = root.join("alpha");
        let child = root.join("child");
        fs::create_dir_all(&alpha).expect("failed to create alpha dir");
        fs::create_dir_all(&child).expect("failed to create child dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(1);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);

        app.go_back().expect("go back should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, root);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(child.as_path())
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn go_forward_reselects_previous_directory_in_parent() {
        let root = temp_path("history-forward-selection");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create child dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);
        app.go_back().expect("go back should succeed");
        wait_for_directory_load(&mut app);

        app.go_forward().expect("go forward should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, child);
        assert!(app.selected_entry().is_none());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn go_forward_restores_last_selected_entry_in_directory() {
        let root = temp_path("history-forward-restore-selection");
        let child = root.join("child");
        let alpha = child.join("alpha.txt");
        let beta = child.join("beta.txt");
        fs::create_dir_all(&child).expect("failed to create child dir");
        fs::write(&alpha, "alpha").expect("failed to write alpha");
        fs::write(&beta, "beta").expect("failed to write beta");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);

        app.select_index(1);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(beta.as_path())
        );

        app.go_back().expect("go back should succeed");
        wait_for_directory_load(&mut app);

        app.go_forward().expect("go forward should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, child);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(beta.as_path())
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn reopening_directory_restores_last_selected_entry() {
        let root = temp_path("reopen-directory-selection");
        let child = root.join("child");
        let alpha = child.join("alpha.txt");
        let beta = child.join("beta.txt");
        fs::create_dir_all(&child).expect("failed to create child dir");
        fs::write(&alpha, "alpha").expect("failed to write alpha");
        fs::write(&beta, "beta").expect("failed to write beta");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);

        app.select_index(1);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(beta.as_path())
        );

        app.go_parent().expect("go parent should succeed");
        wait_for_directory_load(&mut app);
        app.open_selected()
            .expect("reopening selected directory should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, child);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(beta.as_path())
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn reopening_directory_restores_scroll_position() {
        let root = temp_path("reopen-directory-scroll");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create child dir");
        for index in 0..8 {
            fs::write(child.join(format!("file-{index}.txt")), format!("{index}"))
                .expect("failed to write file");
        }

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.set_frame_state(FrameState {
            metrics: ViewMetrics {
                cols: 1,
                rows_visible: 3,
            },
            ..FrameState::default()
        });
        app.select_index(0);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);

        app.select_index(6);
        assert_eq!(app.scroll_row, 4);

        app.go_parent().expect("go parent should succeed");
        wait_for_directory_load(&mut app);
        app.open_selected()
            .expect("reopening selected directory should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, child);
        assert_eq!(app.selected, 6);
        assert_eq!(app.scroll_row, 4);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn reopening_parent_restores_last_selected_child_directory() {
        let root = temp_path("reopen-parent-selection");
        let home = root.join("home");
        let aaa = home.join("aaa");
        let regueiro = home.join("regueiro");
        fs::create_dir_all(&aaa).expect("failed to create aaa dir");
        fs::create_dir_all(&regueiro).expect("failed to create regueiro dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.open_selected().expect("opening home should succeed");
        wait_for_directory_load(&mut app);

        app.select_index(1);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(regueiro.as_path())
        );

        app.open_selected()
            .expect("opening regueiro should succeed");
        wait_for_directory_load(&mut app);
        app.go_parent().expect("go parent to home should succeed");
        wait_for_directory_load(&mut app);
        app.go_parent().expect("go parent to root should succeed");
        wait_for_directory_load(&mut app);

        app.open_selected().expect("reopening home should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, home);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(regueiro.as_path())
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn reopening_parent_restores_scroll_position() {
        let root = temp_path("reopen-parent-scroll");
        let home = root.join("home");
        let child_paths = (0..8)
            .map(|index| home.join(format!("child-{index}")))
            .collect::<Vec<_>>();
        for child in &child_paths {
            fs::create_dir_all(child).expect("failed to create child dir");
        }

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.set_frame_state(FrameState {
            metrics: ViewMetrics {
                cols: 1,
                rows_visible: 3,
            },
            ..FrameState::default()
        });
        app.select_index(0);
        app.open_selected().expect("opening home should succeed");
        wait_for_directory_load(&mut app);

        app.select_index(6);
        assert_eq!(app.scroll_row, 4);

        app.open_selected()
            .expect("opening remembered child should succeed");
        wait_for_directory_load(&mut app);
        app.go_parent().expect("go parent to home should succeed");
        wait_for_directory_load(&mut app);
        app.go_parent().expect("go parent to root should succeed");
        wait_for_directory_load(&mut app);

        app.open_selected().expect("reopening home should succeed");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, home);
        assert_eq!(app.selected, 6);
        assert_eq!(app.scroll_row, 4);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_horizontal_scroll_works_in_list_view() {
        let root = temp_path("preview-horizontal-list");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let file_path = root.join("long.rs");
        fs::write(
            &file_path,
            "fn main() { let preview_line = \"this line is intentionally long for horizontal preview scrolling\"; }\n",
        )
        .expect("failed to write temp file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            preview_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 6,
            preview_cols_visible: 12,
            ..FrameState::default()
        });

        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollRight,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("scroll right should be handled");
        assert!(app.process_pending_scroll());
        assert_eq!(app.preview_horizontal_scroll, 1);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn wheel_burst_smoothing_coalesces_dense_input() {
        let mut lane = ScrollLane::new();

        for _ in 0..6 {
            App::queue_scroll(&mut lane, 1, ENTRY_WHEEL_TUNING);
        }

        assert!(lane.pending.abs() < 6);
        assert!(lane.pending > 0);
    }

    #[test]
    fn browser_wheel_updates_selection_and_preview_immediately() {
        let root = temp_path("wheel-selection-preview");
        fs::create_dir_all(&root).expect("failed to create temp root");
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(root.join(name), name).expect("failed to write temp file");
        }

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            entries_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            metrics: ViewMetrics {
                cols: 1,
                rows_visible: 1,
            },
            ..FrameState::default()
        });
        let initial_preview_token = app.preview_token;

        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("scroll down should be handled");
        assert!(app.process_pending_scroll());

        assert_eq!(app.selected, 1);
        assert_eq!(app.scroll_row, 1);
        assert!(app.preview_token > initial_preview_token);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn browser_wheel_preserves_preview_when_selection_does_not_change() {
        let root = temp_path("wheel-selection-clamp");
        fs::create_dir_all(&root).expect("failed to create temp root");
        for name in ["a.txt", "b.txt"] {
            fs::write(root.join(name), name).expect("failed to write temp file");
        }

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.set_frame_state(FrameState {
            entries_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            metrics: ViewMetrics {
                cols: 1,
                rows_visible: 2,
            },
            ..FrameState::default()
        });
        app.select_index(0);
        let initial_preview_token = app.preview_token;

        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("scroll up should be handled");
        assert!(!app.process_pending_scroll());

        assert_eq!(app.scroll_row, 0);
        assert_eq!(app.selected, 0);
        assert_eq!(app.preview_token, initial_preview_token);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_wheel_uses_last_focused_panel_when_coordinates_miss() {
        let root = temp_path("preview-wheel-focus");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let file_path = root.join("long.txt");
        let contents = (0..40)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&file_path, contents).expect("failed to write temp file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            entries_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_panel: Some(Rect {
                x: 21,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 4,
            preview_cols_visible: 20,
            ..FrameState::default()
        });

        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 22,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("preview click should be handled");
        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 80,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("wheel should fall back to last focused preview panel");

        assert!(app.process_pending_scroll());
        assert!(app.preview_scroll > 0);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_wheel_follows_hovered_panel_without_click() {
        let root = temp_path("preview-wheel-hover");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let file_path = root.join("long.txt");
        let contents = (0..40)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&file_path, contents).expect("failed to write temp file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            entries_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_panel: Some(Rect {
                x: 21,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 4,
            preview_cols_visible: 20,
            ..FrameState::default()
        });

        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 22,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("preview hover should be handled");
        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 80,
            row: 20,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("wheel should fall back to hovered preview panel");

        assert!(app.process_pending_scroll());
        assert!(app.preview_scroll > 0);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_wheel_uses_preview_column_when_row_is_unreliable() {
        let root = temp_path("preview-wheel-column-fallback");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let file_path = root.join("long.txt");
        let contents = (0..40)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&file_path, contents).expect("failed to write temp file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            entries_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_panel: Some(Rect {
                x: 21,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 4,
            preview_cols_visible: 20,
            ..FrameState::default()
        });

        app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 22,
            row: 20,
            modifiers: KeyModifiers::NONE,
        }))
        .expect("wheel should use preview column fallback");

        assert!(app.process_pending_scroll());
        assert!(app.preview_scroll > 0);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn high_frequency_alt_right_scrolls_preview_instead_of_history() {
        let root = temp_path("preview-horizontal-alt-right");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let file_path = root.join("long.rs");
        fs::write(
            &file_path,
            "fn main() { let preview_line = \"this line is intentionally long for horizontal preview scrolling\"; }\n",
        )
        .expect("failed to write temp file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.wheel_profile = WheelProfile::HighFrequency;
        app.last_wheel_target = Some(WheelTarget::Entries);
        app.select_index(0);
        app.last_selection_change_at =
            Instant::now() - PREVIEW_AUTO_FOCUS_DELAY - Duration::from_millis(1);
        app.set_frame_state(FrameState {
            preview_panel: Some(Rect {
                x: 21,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 6,
            preview_cols_visible: 12,
            ..FrameState::default()
        });

        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)))
            .expect("alt-right should be handled");

        assert!(app.preview_horizontal_scroll > 0);
        assert_eq!(app.selected, 0);
        assert_eq!(app.last_wheel_target, Some(WheelTarget::Preview));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn high_frequency_down_arrow_keeps_browser_navigation() {
        let root = temp_path("high-frequency-down-keeps-browser");
        fs::create_dir_all(&root).expect("failed to create temp root");
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(root.join(name), name).expect("failed to write temp file");
        }

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.wheel_profile = WheelProfile::HighFrequency;
        app.select_index(0);
        app.last_wheel_target = Some(WheelTarget::Preview);
        app.last_selection_change_at =
            Instant::now() - PREVIEW_AUTO_FOCUS_DELAY - Duration::from_millis(1);
        app.set_frame_state(FrameState {
            preview_panel: Some(Rect {
                x: 21,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 4,
            preview_cols_visible: 20,
            ..FrameState::default()
        });

        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))
            .expect("down arrow should be handled");

        assert_eq!(app.selected, 1);
        assert_eq!(app.preview_scroll, 0);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn high_frequency_right_arrow_in_list_view_still_enters_directory() {
        let root = temp_path("high-frequency-right-enters");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create child dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.wheel_profile = WheelProfile::HighFrequency;
        app.select_index(0);
        app.last_wheel_target = Some(WheelTarget::Preview);

        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::NONE,
        )))
        .expect("right arrow should be handled");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, child);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn high_frequency_alt_right_does_not_trigger_history_navigation() {
        let root = temp_path("high-frequency-alt-right-no-history");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create child dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.wheel_profile = WheelProfile::HighFrequency;
        app.select_index(0);
        app.open_selected()
            .expect("opening selected directory should succeed");
        wait_for_directory_load(&mut app);
        app.go_back().expect("go back should succeed");
        wait_for_directory_load(&mut app);

        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)))
            .expect("alt-right should be handled");

        assert_eq!(app.cwd, root);
        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(child.as_path())
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_scroll_resets_when_reselecting_a_file() {
        let root = temp_path("preview-scroll-restore");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let long = root.join("a.txt");
        let other = root.join("b.txt");
        let contents = (0..24)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&long, contents).expect("failed to write long text file");
        fs::write(&other, "short\ntext").expect("failed to write other text file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            preview_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 8,
            }),
            preview_rows_visible: 4,
            preview_cols_visible: 40,
            ..FrameState::default()
        });

        app.preview_scroll = 5;
        app.sync_preview_scroll();
        assert_eq!(app.preview_scroll, 5);

        app.select_index(1);
        app.select_index(0);

        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(long.as_path())
        );
        assert_eq!(app.preview_scroll, 0);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_horizontal_scroll_resets_when_reselecting_code() {
        let root = temp_path("preview-horizontal-restore");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let code = root.join("a.rs");
        let other = root.join("b.txt");
        fs::write(
            &code,
            "fn main() { let preview_line = \"this line is intentionally long for horizontal preview scrolling\"; }\n",
        )
        .expect("failed to write code file");
        fs::write(&other, "short\ntext").expect("failed to write other text file");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        app.view_mode = ViewMode::List;
        app.select_index(0);
        app.set_frame_state(FrameState {
            preview_panel: Some(Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 8,
            }),
            preview_rows_visible: 6,
            preview_cols_visible: 12,
            ..FrameState::default()
        });

        app.preview_horizontal_scroll = 3;
        app.sync_preview_scroll();
        assert_eq!(app.preview_horizontal_scroll, 3);

        app.select_index(1);
        app.select_index(0);

        assert_eq!(
            app.selected_entry().map(|entry| entry.path.as_path()),
            Some(code.as_path())
        );
        assert_eq!(app.preview_horizontal_scroll, 0);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn opening_a_removed_directory_does_not_bubble_an_error() {
        let root = temp_path("removed-directory-open");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create temp dirs");

        let mut app = App::new_at(root.clone()).expect("failed to create app");
        fs::remove_dir_all(&child).expect("failed to remove child dir");

        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
        .expect("stale directory open should be handled");

        assert_eq!(app.cwd, root);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    #[cfg(unix)]
    fn opening_a_protected_directory_reports_permission_denied() {
        let root = temp_path("protected-directory-open");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("failed to create temp dirs");
        fs::set_permissions(&child, fs::Permissions::from_mode(0o000))
            .expect("failed to lock child dir");

        let mut app = App::new_at(root.clone()).expect("failed to create app");

        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
        .expect("protected directory open should be handled");
        wait_for_directory_load(&mut app);

        assert_eq!(app.cwd, root);
        assert!(app.status_message().contains("Permission denied"));

        fs::set_permissions(&child, fs::Permissions::from_mode(0o755))
            .expect("failed to unlock child dir");
        fs::remove_dir_all(root).expect("failed to remove temp root");
    }
}
