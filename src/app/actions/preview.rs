use super::*;
use crate::preview::{
    PreviewContent, PreviewKind, PreviewRequestOptions, build_preview_with_options,
    loading_preview_for, should_build_preview_in_background,
};
use std::sync::Arc;

impl App {
    pub fn preview_lines(&self) -> Vec<Line<'static>> {
        self.preview_state.content.lines()
    }

    pub fn preview_wrapped_lines(&self, visible_cols: usize) -> Arc<[Line<'static>]> {
        self.preview_state.content.wrapped_lines(visible_cols)
    }

    pub fn preview_section_label(&self) -> &'static str {
        self.preview_state.content.section_label()
    }

    pub fn preview_scroll_offset(&self) -> usize {
        self.preview_state.scroll
    }

    pub fn preview_horizontal_scroll_offset(&self) -> usize {
        self.preview_state.horizontal_scroll
    }

    pub fn preview_total_lines(&self, visible_cols: usize) -> usize {
        self.preview_state.content.visual_line_count(visible_cols)
    }

    pub fn preview_wraps(&self) -> bool {
        self.preview_state.content.kind.wraps_in_preview()
    }

    pub fn preview_allows_horizontal_scroll(&self) -> bool {
        self.preview_state.content.kind.allows_horizontal_scroll()
    }

    pub fn preview_max_horizontal_scroll(&self, visible_cols: usize) -> usize {
        if !self.preview_allows_horizontal_scroll() {
            return 0;
        }
        self.preview_state
            .content
            .max_line_width()
            .saturating_sub(visible_cols.max(1))
    }

    pub fn preview_header_detail(&self, visible_rows: usize) -> Option<String> {
        let visible_cols = self.frame_state.preview_cols_visible;
        let detail = self
            .preview_state
            .content
            .header_detail(self.preview_state.scroll, visible_rows);
        let wrapped_note =
            if self.preview_state.content.truncation_note.is_none() && visible_cols > 0 {
                self.preview_state
                    .content
                    .wrapped_truncation_note(visible_cols)
            } else {
                None
            };
        let mut detail = match (detail, wrapped_note) {
            (Some(detail), Some(note)) if !note.is_empty() => Some(format!("{detail}  •  {note}")),
            (Some(detail), Some(_)) => Some(detail),
            (Some(detail), None) => Some(detail),
            (None, Some(note)) => Some(note),
            (None, None) => None,
        };
        if let Some(navigation_detail) = self.preview_state.content.navigation_header_detail() {
            detail = Some(match detail {
                Some(detail) if !detail.is_empty() => format!("{detail}  •  {navigation_detail}"),
                _ => navigation_detail,
            });
        }
        if let Some(pdf_detail) = self.pdf_preview_header_detail() {
            detail = Some(match detail {
                Some(detail) if !detail.is_empty() => format!("{detail}  •  {pdf_detail}"),
                _ => pdf_detail,
            });
        }
        if let Some(image_detail) = self.static_image_preview_header_detail() {
            detail = Some(match detail {
                Some(detail) if !detail.is_empty() => format!("{detail}  •  {image_detail}"),
                _ => image_detail,
            });
        }
        detail
    }

    pub(in crate::app) fn current_preview_request_options(&self) -> PreviewRequestOptions {
        self.comic_preview_request_options()
            .or_else(|| self.epub_preview_request_options())
            .unwrap_or_default()
    }

    pub(in crate::app) fn refresh_preview(&mut self) {
        self.preview_state.deferred_refresh_at = None;
        self.sync_comic_preview_selection();
        self.sync_epub_preview_selection();
        self.sync_pdf_preview_selection();
        self.sync_image_preview_selection_activation();
        self.preview_state.token = self.preview_state.token.wrapping_add(1);
        let preview_options = self.current_preview_request_options();
        self.preview_state.content = match self.selected_entry().cloned() {
            Some(entry) if self.should_defer_static_image_preview(&entry) => {
                self.preview_state.load_state = None;
                PreviewContent::new(PreviewKind::Image, Vec::new()).with_detail(
                    self.static_image_preview_detail(&entry)
                        .unwrap_or("Image preview"),
                )
            }
            Some(entry) if self.should_defer_pdf_document_preview(&entry) => {
                self.preview_state.load_state = None;
                self.cached_preview_for(&entry, &preview_options)
                    .or_else(|| self.stale_cached_preview_for(&entry, &preview_options))
                    .unwrap_or_else(|| {
                        PreviewContent::new(PreviewKind::Document, Vec::new())
                            .with_detail("PDF document")
                    })
            }
            Some(entry) if should_build_preview_in_background(&entry) => {
                if let Some(preview) = self.cached_preview_for(&entry, &preview_options) {
                    self.preview_state.metrics.cache_hits += 1;
                    self.preview_state.load_state = None;
                    preview
                } else if let Some(stale_preview) =
                    self.stale_cached_preview_for(&entry, &preview_options)
                {
                    self.preview_state.metrics.cache_misses += 1;
                    let loading_path = entry.path.clone();
                    let request = PreviewRequest {
                        token: self.preview_state.token,
                        entry,
                        variant: preview_options.clone(),
                        priority: PreviewPriority::High,
                    };
                    if !self.scheduler.submit_preview(request) {
                        self.preview_state.load_state = None;
                        stale_preview.with_status_note("Refresh unavailable")
                    } else {
                        self.preview_state.load_state =
                            Some(PreviewLoadState::Refreshing(loading_path));
                        stale_preview.with_status_note("Refreshing in background")
                    }
                } else {
                    self.preview_state.metrics.cache_misses += 1;
                    let placeholder = loading_preview_for(&entry, &preview_options);
                    let loading_path = entry.path.clone();
                    let request = PreviewRequest {
                        token: self.preview_state.token,
                        entry,
                        variant: preview_options.clone(),
                        priority: PreviewPriority::High,
                    };
                    if !self.scheduler.submit_preview(request) {
                        self.preview_state.load_state = None;
                        PreviewContent::placeholder("Preview worker unavailable")
                    } else {
                        self.preview_state.load_state =
                            Some(PreviewLoadState::Placeholder(loading_path));
                        placeholder
                    }
                }
            }
            Some(entry) => {
                self.preview_state.load_state = None;
                build_preview_with_options(&entry, &preview_options)
            }
            None => {
                self.preview_state.load_state = None;
                PreviewContent::placeholder("No selection")
            }
        };
        self.apply_current_comic_preview_metadata();
        self.apply_current_epub_preview_metadata();
        self.preview_state.scroll = 0;
        self.preview_state.horizontal_scroll = 0;
        self.sync_preview_scroll();
        self.refresh_static_image_preloads();
        self.prefetch_nearby_previews();
    }

    pub(crate) fn process_preview_refresh_timers(&mut self) -> bool {
        let Some(deadline) = self.preview_state.deferred_refresh_at else {
            return false;
        };
        if Instant::now() < deadline {
            return false;
        }
        self.refresh_preview();
        true
    }

    pub(crate) fn pending_preview_refresh_timer(&self) -> Option<std::time::Duration> {
        self.preview_state
            .deferred_refresh_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn cached_preview_for(
        &self,
        entry: &Entry,
        variant: &PreviewRequestOptions,
    ) -> Option<PreviewContent> {
        let cached = self.preview_state.result_cache.get(&PreviewCacheKey {
            path: entry.path.clone(),
            variant: variant.clone(),
        })?;
        if cached.size == entry.size && cached.modified == entry.modified {
            Some(cached.preview.clone())
        } else {
            None
        }
    }

    fn stale_cached_preview_for(
        &self,
        entry: &Entry,
        variant: &PreviewRequestOptions,
    ) -> Option<PreviewContent> {
        self.preview_state
            .result_cache
            .get(&PreviewCacheKey {
                path: entry.path.clone(),
                variant: variant.clone(),
            })
            .map(|cached| cached.preview.clone())
    }

    pub(in crate::app) fn cache_preview_result(
        &mut self,
        entry: &Entry,
        variant: &PreviewRequestOptions,
        preview: &PreviewContent,
    ) {
        let key = PreviewCacheKey {
            path: entry.path.clone(),
            variant: variant.clone(),
        };
        self.preview_state.result_cache.insert(
            key.clone(),
            CachedPreview {
                size: entry.size,
                modified: entry.modified,
                preview: preview.clone(),
            },
        );
        self.preview_state.result_order.retain(|cached| cached != &key);
        self.preview_state.result_order.push_back(key);

        while self.preview_state.result_order.len() > PREVIEW_CACHE_LIMIT {
            if let Some(stale_key) = self.preview_state.result_order.pop_front() {
                self.preview_state.result_cache.remove(&stale_key);
            }
        }
    }

    fn prefetch_nearby_previews(&mut self) {
        let mut queued = 0;
        for offset in [1isize, -1, 2, -2, 3, -3] {
            if queued >= PREVIEW_PREFETCH_LIMIT {
                break;
            }

            let target = self.selected as isize + offset;
            if target < 0 {
                continue;
            }
            let Some(entry) = self.entries.get(target as usize).cloned() else {
                continue;
            };
            let variant = self.preview_request_options_for_entry(&entry);
            if !should_build_preview_in_background(&entry)
                || self.cached_preview_for(&entry, &variant).is_some()
            {
                continue;
            }

            let request = PreviewRequest {
                token: self.preview_state.token,
                entry,
                variant,
                priority: PreviewPriority::Low,
            };
            if self.scheduler.submit_preview(request) {
                queued += 1;
            }
        }
    }

    fn preview_request_options_for_entry(&self, entry: &Entry) -> PreviewRequestOptions {
        self.comic_preview_request_options_for_entry(entry)
            .or_else(|| self.epub_preview_request_options_for_entry(entry))
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::app) fn has_cached_preview_for_path(&self, path: &std::path::Path) -> bool {
        self.preview_state
            .result_cache
            .keys()
            .any(|key| key.path == path)
    }
}
