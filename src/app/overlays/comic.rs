use super::super::*;
use std::{
    path::PathBuf,
    time::{Instant, SystemTime},
};

#[derive(Clone, Debug, Default)]
pub(in crate::app) struct ComicPreviewState {
    session: Option<ComicSession>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ComicSession {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    current_page: usize,
    total_pages: Option<usize>,
}

impl App {
    pub(in crate::app) fn sync_comic_preview_selection(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.comic_preview.session = None;
            return;
        };
        if !is_comic_entry(entry) {
            self.comic_preview.session = None;
            return;
        }

        let keep_session = self.comic_preview.session.as_ref().is_some_and(|session| {
            session.path == entry.path
                && session.size == entry.size
                && session.modified == entry.modified
        });
        if keep_session {
            return;
        }

        self.comic_preview.session = Some(ComicSession {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
            current_page: 0,
            total_pages: self.cached_comic_page_count(entry),
        });
    }

    pub(in crate::app) fn comic_preview_request_options(
        &self,
    ) -> Option<preview::PreviewRequestOptions> {
        self.comic_preview
            .session
            .as_ref()
            .map(|session| preview::PreviewRequestOptions::ComicPage(session.current_page))
    }

    pub(in crate::app) fn comic_preview_request_options_for_entry(
        &self,
        entry: &Entry,
    ) -> Option<preview::PreviewRequestOptions> {
        is_comic_entry(entry).then_some(preview::PreviewRequestOptions::ComicPage(0))
    }

    pub(in crate::app) fn apply_current_comic_preview_metadata(&mut self) {
        let Some((path, size, modified)) = self
            .selected_entry()
            .map(|entry| (entry.path.clone(), entry.size, entry.modified))
        else {
            return;
        };
        let Some(session) = self.comic_preview.session.as_mut() else {
            return;
        };
        if session.path != path || session.size != size || session.modified != modified {
            return;
        }

        let Some(position) = self.preview_state.content.navigation_position.as_ref() else {
            return;
        };
        if position.label != "Page" {
            return;
        }

        session.total_pages = Some(position.count);
        session.current_page = position.index;
    }

    pub(in crate::app) fn step_comic_page(&mut self, delta: isize) -> bool {
        let Some(session) = self.comic_preview.session.as_mut() else {
            return false;
        };
        let total_pages = session
            .total_pages
            .or(self
                .preview_state
                .content
                .navigation_position
                .as_ref()
                .filter(|position| position.label == "Page")
                .map(|position| position.count))
            .unwrap_or(0);
        if total_pages == 0 {
            return false;
        }

        let previous = session.current_page;
        let next = if delta.is_negative() {
            previous.saturating_sub(delta.unsigned_abs())
        } else {
            previous.saturating_add(delta as usize)
        };
        session.current_page = next.min(total_pages.saturating_sub(1));
        if session.current_page == previous {
            return false;
        }

        self.preview_state.deferred_refresh_at = Some(Instant::now());
        self.refresh_preview();
        true
    }

    fn cached_comic_page_count(&self, entry: &Entry) -> Option<usize> {
        self.preview_state.result_cache.iter().find_map(|(key, cached)| {
            (key.path == entry.path
                && cached.size == entry.size
                && cached.modified == entry.modified)
                .then(|| cached.preview.navigation_position.as_ref())
                .flatten()
                .filter(|position| position.label == "Page")
                .map(|position| position.count)
        })
    }
}

fn is_comic_entry(entry: &Entry) -> bool {
    entry.path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cbz"))
}
