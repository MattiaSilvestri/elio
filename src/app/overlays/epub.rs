use super::super::*;
use crate::file_info::{self, DocumentFormat};
use std::{
    path::PathBuf,
    time::{Instant, SystemTime},
};

#[derive(Clone, Debug, Default)]
pub(in crate::app) struct EpubPreviewState {
    session: Option<EpubSession>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EpubSession {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    current_section: usize,
    total_sections: Option<usize>,
}

impl App {
    pub(in crate::app) fn sync_epub_preview_selection(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.epub_preview.session = None;
            return;
        };
        if !is_epub_entry(entry) {
            self.epub_preview.session = None;
            return;
        }

        let keep_session = self.epub_preview.session.as_ref().is_some_and(|session| {
            session.path == entry.path
                && session.size == entry.size
                && session.modified == entry.modified
        });
        if keep_session {
            return;
        }

        self.epub_preview.session = Some(EpubSession {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
            current_section: 0,
            total_sections: self.cached_epub_section_count(entry),
        });
    }

    pub(in crate::app) fn epub_preview_request_options(
        &self,
    ) -> Option<preview::PreviewRequestOptions> {
        self.epub_preview
            .session
            .as_ref()
            .map(|session| preview::PreviewRequestOptions::EpubSection(session.current_section))
    }

    pub(in crate::app) fn epub_preview_request_options_for_entry(
        &self,
        entry: &Entry,
    ) -> Option<preview::PreviewRequestOptions> {
        is_epub_entry(entry).then_some(preview::PreviewRequestOptions::EpubSection(0))
    }

    pub(in crate::app) fn apply_current_epub_preview_metadata(&mut self) {
        let Some((path, size, modified)) = self
            .selected_entry()
            .map(|entry| (entry.path.clone(), entry.size, entry.modified))
        else {
            return;
        };
        let Some(session) = self.epub_preview.session.as_mut() else {
            return;
        };
        if session.path != path || session.size != size || session.modified != modified {
            return;
        }

        if let Some(total_sections) = self.preview_state.content.ebook_section_count {
            session.total_sections = Some(total_sections);
        }
        if let Some(section_index) = self.preview_state.content.ebook_section_index {
            session.current_section = section_index;
        }
    }

    pub(in crate::app) fn step_epub_section(&mut self, delta: isize) -> bool {
        let Some(session) = self.epub_preview.session.as_mut() else {
            return false;
        };
        let total_sections = session
            .total_sections
            .or(self.preview_state.content.ebook_section_count)
            .unwrap_or(0);
        if total_sections == 0 {
            return false;
        }

        let previous = session.current_section;
        let next = if delta.is_negative() {
            previous.saturating_sub(delta.unsigned_abs())
        } else {
            previous.saturating_add(delta as usize)
        };
        session.current_section = next.min(total_sections.saturating_sub(1));
        if session.current_section == previous {
            return false;
        }

        self.preview_state.deferred_refresh_at = Some(Instant::now());
        self.refresh_preview();
        true
    }

    fn cached_epub_section_count(&self, entry: &Entry) -> Option<usize> {
        self.preview_state
            .result_cache
            .iter()
            .find_map(|(key, cached)| {
                (key.path == entry.path
                    && cached.size == entry.size
                    && cached.modified == entry.modified)
                    .then_some(cached.preview.ebook_section_count)
                    .flatten()
            })
    }

}

fn is_epub_entry(entry: &Entry) -> bool {
    file_info::inspect_path_cached(&entry.path, entry.kind, entry.size, entry.modified)
        .preview
        .document_format
        == Some(DocumentFormat::Epub)
}
