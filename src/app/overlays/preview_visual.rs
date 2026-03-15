use super::super::*;
use super::images;
use ratatui::layout::Rect;

const PREVIEW_INLINE_COVER_MIN_HEIGHT: u16 = 6;
const PREVIEW_INLINE_COVER_MAX_HEIGHT: u16 = 12;
const PREVIEW_INLINE_MIN_TEXT_HEIGHT: u16 = 6;
const PREVIEW_INLINE_PAGE_MIN_HEIGHT: u16 = 8;
const PREVIEW_INLINE_PAGE_MIN_TEXT_HEIGHT: u16 = 6;

impl App {
    pub(crate) fn preview_visual_rows(&self, area: Rect) -> Option<u16> {
        if !self.terminal_image_overlay_available() || self.preview_state.content.preview_visual.is_none() {
            return None;
        }
        match self.current_preview_visual_layout()? {
            preview::PreviewVisualLayout::FullHeight => {
                return (area.width >= 12 && area.height > 0).then_some(area.height);
            }
            preview::PreviewVisualLayout::Inline => {}
        }
        if self.current_preview_visual_kind() == Some(preview::PreviewVisualKind::PageImage) {
            if area.width < 12
                || area.height < PREVIEW_INLINE_PAGE_MIN_HEIGHT + PREVIEW_INLINE_PAGE_MIN_TEXT_HEIGHT
            {
                return None;
            }
            return Some(area.height.saturating_sub(PREVIEW_INLINE_PAGE_MIN_TEXT_HEIGHT));
        }
        if area.width < 12
            || area.height < PREVIEW_INLINE_COVER_MIN_HEIGHT + PREVIEW_INLINE_MIN_TEXT_HEIGHT
        {
            return None;
        }

        Some(
            (area.height / 3)
                .clamp(PREVIEW_INLINE_COVER_MIN_HEIGHT, PREVIEW_INLINE_COVER_MAX_HEIGHT)
                .min(area.height.saturating_sub(PREVIEW_INLINE_MIN_TEXT_HEIGHT)),
        )
    }

    pub(crate) fn preview_visual_placeholder_message(&self) -> Option<String> {
        let request = self.active_preview_visual_overlay_request()?;
        let key = images::StaticImageKey::from_request(&request);
        if self.image_preview.failed_images.contains(&key) {
            return None;
        }
        if self.image_preview.dimensions.contains_key(&key) {
            return None;
        }
        if self.current_preview_visual_kind() == Some(preview::PreviewVisualKind::PageImage) {
            return Some("Preparing page preview".to_string());
        }
        if self.image_preview.pending_prepares.contains(&key) {
            return Some("Preparing cover preview".to_string());
        }
        Some("Preparing cover preview".to_string())
    }

    pub(in crate::app) fn active_preview_visual_overlay_request(
        &self,
    ) -> Option<images::StaticImageOverlayRequest> {
        if self.preview_uses_image_overlay() {
            return None;
        }

        self.active_preview_visual_overlay_request_unchecked()
    }

    pub(in crate::app) fn active_preview_visual_overlay_request_unchecked(
        &self,
    ) -> Option<images::StaticImageOverlayRequest> {
        if !self.terminal_image_overlay_available() {
            return None;
        }

        let visual = self.preview_state.content.preview_visual.as_ref()?;
        let area = self.frame_state.preview_media_area?;
        if area.width == 0 || area.height == 0 {
            return None;
        }

        Some(images::StaticImageOverlayRequest {
            path: visual.path.clone(),
            size: visual.size,
            modified: visual.modified,
            area,
            target_width_px: images::image_target_width_px(area, self.cached_terminal_window()),
            target_height_px: images::image_target_height_px(area, self.cached_terminal_window()),
            mode: images::StaticImageOverlayMode::Inline,
        })
    }

    fn current_preview_visual_kind(&self) -> Option<preview::PreviewVisualKind> {
        self.preview_state
            .content
            .preview_visual
            .as_ref()
            .map(|visual| visual.kind)
    }

    fn current_preview_visual_layout(&self) -> Option<preview::PreviewVisualLayout> {
        self.preview_state
            .content
            .preview_visual
            .as_ref()
            .map(|visual| visual.layout)
    }
}

#[cfg(test)]
mod tests;
