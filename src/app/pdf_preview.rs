use super::*;
use crate::file_facts::{self, DocumentFormat};
use anyhow::{Context, Result};
use crossterm::terminal;
use ratatui::layout::Rect;
use std::{
    collections::{hash_map::DefaultHasher, HashMap, VecDeque},
    env, fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const PDF_RENDER_CACHE_LIMIT: usize = 12;
const PDF_RENDER_MIN_DIMENSION_PX: u32 = 96;
const PDF_PAGE_MIN: usize = 1;

#[derive(Clone, Debug, Default)]
pub(super) struct PdfPreviewState {
    enabled: bool,
    session: Option<PdfSession>,
    page_dimensions: HashMap<PdfPageKey, PdfPageDimensions>,
    rendered_pages: HashMap<PdfRenderKey, PathBuf>,
    render_order: VecDeque<PdfRenderKey>,
    displayed: Option<DisplayedPdfPreview>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PdfSession {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    current_page: usize,
    total_pages: Option<usize>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PdfPageKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    page: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PdfPageDimensions {
    width_pts: f32,
    height_pts: f32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PdfRenderKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    page: usize,
    width_px: u32,
    height_px: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayedPdfPreview {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    page: usize,
    area: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminalWindowSize {
    cells_width: u16,
    cells_height: u16,
    pixels_width: u32,
    pixels_height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PdfOverlayRequest {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    page: usize,
    area: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FittedPdfPlacement {
    image_area: Rect,
    render_width_px: u32,
    render_height_px: u32,
}

impl App {
    pub(crate) fn enable_terminal_pdf_previews(&mut self) {
        self.pdf_preview.enabled = detect_terminal_pdf_preview_support();
        self.sync_pdf_preview_selection();
    }

    pub(crate) fn present_pdf_overlay(&mut self) -> Result<()> {
        let Some(request) = self.active_pdf_overlay_request() else {
            self.clear_pdf_overlay()?;
            return Ok(());
        };

        if self.pdf_preview.displayed.as_ref() == Some(&DisplayedPdfPreview::from_request(&request))
        {
            return Ok(());
        }

        let Some(window_size) = query_terminal_window_size() else {
            self.clear_pdf_overlay()?;
            return Ok(());
        };

        let Some(page_dimensions) = self.page_dimensions_for(&request) else {
            self.clear_pdf_overlay()?;
            return Ok(());
        };
        let placement = fit_pdf_page(request.area, window_size, page_dimensions);
        let render_key = PdfRenderKey::from_request(&request, placement);
        let rendered = match self.ensure_pdf_render(&render_key) {
            Ok(Some(path)) => path,
            Ok(None) => {
                self.clear_pdf_overlay()?;
                return Ok(());
            }
            Err(error) => {
                self.status = format!("PDF preview unavailable: {error}");
                self.clear_pdf_overlay()?;
                return Ok(());
            }
        };

        if self.pdf_preview.displayed.is_some() {
            clear_pdf_images().context("failed to clear previous PDF page")?;
            self.pdf_preview.displayed = None;
        }
        place_pdf_image(&rendered, placement.image_area).context("failed to display PDF page")?;
        self.pdf_preview.displayed = Some(DisplayedPdfPreview::from_request(&request));
        Ok(())
    }

    pub(crate) fn clear_pdf_overlay(&mut self) -> Result<()> {
        if self.pdf_preview.displayed.is_none() {
            return Ok(());
        }
        clear_pdf_images().context("failed to clear PDF preview overlay")?;
        self.pdf_preview.displayed = None;
        Ok(())
    }

    pub(crate) fn preview_uses_image_overlay(&self) -> bool {
        self.active_pdf_overlay_request().is_some() && self.pdf_preview.displayed.is_some()
    }

    pub(super) fn pdf_preview_header_detail(&self) -> Option<String> {
        let session = self.pdf_preview.session.as_ref()?;
        if !self.pdf_preview.enabled {
            return None;
        }

        let page_label = match session.total_pages {
            Some(total_pages) => format!("Page {}/{}", session.current_page, total_pages),
            None => format!("Page {}", session.current_page),
        };
        Some(page_label)
    }

    pub(super) fn step_pdf_page(&mut self, delta: isize) -> bool {
        let Some(session) = &mut self.pdf_preview.session else {
            return false;
        };

        let previous_page = session.current_page;
        let next_page = if delta.is_negative() {
            session.current_page.saturating_sub(delta.unsigned_abs())
        } else {
            session.current_page.saturating_add(delta as usize)
        };

        let max_page = session.total_pages.unwrap_or(next_page.max(PDF_PAGE_MIN));
        session.current_page = next_page.clamp(PDF_PAGE_MIN, max_page.max(PDF_PAGE_MIN));
        if session.current_page == previous_page {
            return false;
        }

        self.status = match session.total_pages {
            Some(total_pages) => format!("PDF page {}/{}", session.current_page, total_pages),
            None => format!("PDF page {}", session.current_page),
        };
        true
    }

    pub(super) fn sync_pdf_preview_selection(&mut self) {
        if !self.pdf_preview.enabled {
            self.pdf_preview.session = None;
            return;
        }

        let Some(entry) = self.selected_entry() else {
            self.pdf_preview.session = None;
            return;
        };
        if !is_pdf_entry(entry) {
            self.pdf_preview.session = None;
            return;
        }

        let should_keep_session = self.pdf_preview.session.as_ref().is_some_and(|session| {
            session.path == entry.path
                && session.size == entry.size
                && session.modified == entry.modified
        });
        if should_keep_session {
            return;
        }

        self.pdf_preview.session = Some(PdfSession {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
            current_page: PDF_PAGE_MIN,
            total_pages: query_pdf_page_count(&entry.path),
        });
    }

    fn active_pdf_overlay_request(&self) -> Option<PdfOverlayRequest> {
        if !self.pdf_preview.enabled {
            return None;
        }

        let session = self.pdf_preview.session.as_ref()?;
        let area = self.frame_state.preview_content_area?;
        if area.width == 0 || area.height == 0 {
            return None;
        }

        Some(PdfOverlayRequest {
            path: session.path.clone(),
            size: session.size,
            modified: session.modified,
            page: session.current_page,
            area,
        })
    }

    fn ensure_pdf_render(&mut self, key: &PdfRenderKey) -> Result<Option<PathBuf>> {
        if let Some(path) = self.pdf_preview.rendered_pages.get(key)
            && path.exists()
        {
            return Ok(Some(path.clone()));
        }

        let cache_dir = pdf_render_cache_dir()?;
        let stem = cache_dir.join(pdf_render_cache_stem(key));
        let png_path = stem.with_extension("png");

        let status = Command::new("pdftocairo")
            .arg("-png")
            .arg("-singlefile")
            .arg("-f")
            .arg(key.page.to_string())
            .arg("-l")
            .arg(key.page.to_string())
            .arg("-scale-to-x")
            .arg(key.width_px.to_string())
            .arg("-scale-to-y")
            .arg("-1")
            .arg(&key.path)
            .arg(&stem)
            .status()
            .context("failed to start pdftocairo")?;

        if !status.success() || !png_path.exists() {
            return Ok(None);
        }

        self.pdf_preview
            .rendered_pages
            .insert(key.clone(), png_path.clone());
        self.pdf_preview.render_order.retain(|queued| queued != key);
        self.pdf_preview.render_order.push_back(key.clone());
        while self.pdf_preview.render_order.len() > PDF_RENDER_CACHE_LIMIT {
            if let Some(stale_key) = self.pdf_preview.render_order.pop_front()
                && let Some(stale_path) = self.pdf_preview.rendered_pages.remove(&stale_key)
            {
                let _ = fs::remove_file(stale_path);
            }
        }

        Ok(Some(png_path))
    }

    fn page_dimensions_for(&mut self, request: &PdfOverlayRequest) -> Option<PdfPageDimensions> {
        let key = PdfPageKey::from_request(request);
        if let Some(dimensions) = self.pdf_preview.page_dimensions.get(&key).copied() {
            return Some(dimensions);
        }

        let dimensions = query_pdf_page_dimensions(&request.path, request.page)?;
        self.pdf_preview.page_dimensions.insert(key, dimensions);
        Some(dimensions)
    }
}

impl PdfRenderKey {
    fn from_request(request: &PdfOverlayRequest, placement: FittedPdfPlacement) -> Self {
        Self {
            path: request.path.clone(),
            size: request.size,
            modified: request.modified,
            page: request.page,
            width_px: placement.render_width_px,
            height_px: placement.render_height_px,
        }
    }
}

impl PdfPageKey {
    fn from_request(request: &PdfOverlayRequest) -> Self {
        Self {
            path: request.path.clone(),
            size: request.size,
            modified: request.modified,
            page: request.page,
        }
    }
}

impl DisplayedPdfPreview {
    fn from_request(request: &PdfOverlayRequest) -> Self {
        Self {
            path: request.path.clone(),
            size: request.size,
            modified: request.modified,
            page: request.page,
            area: request.area,
        }
    }
}

fn detect_terminal_pdf_preview_support() -> bool {
    if env::var_os("KITTY_WINDOW_ID").is_none()
        || !command_exists("kitten")
        || !command_exists("pdftocairo")
    {
        return false;
    }

    Command::new("kitten")
        .arg("icat")
        .arg("--stdin=no")
        .arg("--detect-support")
        .arg("--detection-timeout=1")
        .status()
        .is_ok_and(|status| status.success())
}

fn command_exists(program: &str) -> bool {
    Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {program} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn is_pdf_entry(entry: &Entry) -> bool {
    file_facts::inspect_path(&entry.path, entry.kind)
        .preview
        .document_format
        == Some(DocumentFormat::Pdf)
}

fn query_pdf_page_count(path: &Path) -> Option<usize> {
    let output = Command::new("pdfinfo").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_pdfinfo_page_count(&String::from_utf8_lossy(&output.stdout))
}

fn query_pdf_page_dimensions(path: &Path, page: usize) -> Option<PdfPageDimensions> {
    let output = Command::new("pdfinfo")
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_pdfinfo_page_dimensions(&String::from_utf8_lossy(&output.stdout))
}

fn parse_pdfinfo_page_count(output: &str) -> Option<usize> {
    output.lines().find_map(|line| {
        let (label, value) = line.split_once(':')?;
        (label.trim() == "Pages")
            .then_some(value.trim())
            .and_then(|value| value.parse().ok())
    })
}

fn parse_pdfinfo_page_dimensions(output: &str) -> Option<PdfPageDimensions> {
    output.lines().find_map(|line| {
        let (label, value) = line.split_once(':')?;
        let label = label.trim();
        if !(label == "Page size" || label.starts_with("Page ") && label.ends_with(" size")) {
            return None;
        }

        let mut parts = value.split_whitespace();
        let width_pts = parts.next()?.parse().ok()?;
        let _separator = parts.next()?;
        let height_pts = parts.next()?.parse().ok()?;
        Some(PdfPageDimensions {
            width_pts,
            height_pts,
        })
    })
}

fn query_terminal_window_size() -> Option<TerminalWindowSize> {
    let (cells_width, cells_height) = terminal::size().ok()?;
    let output = Command::new("kitten")
        .arg("icat")
        .arg("--print-window-size")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let (pixels_width, pixels_height) =
        parse_window_size(&String::from_utf8_lossy(&output.stdout))?;
    Some(TerminalWindowSize {
        cells_width,
        cells_height,
        pixels_width,
        pixels_height,
    })
}

fn parse_window_size(output: &str) -> Option<(u32, u32)> {
    let trimmed = output.trim();
    let (width, height) = trimmed.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn fit_pdf_page(
    area: Rect,
    window_size: TerminalWindowSize,
    page_dimensions: PdfPageDimensions,
) -> FittedPdfPlacement {
    let cell_width_px = window_size.pixels_width as f32 / f32::from(window_size.cells_width.max(1));
    let cell_height_px =
        window_size.pixels_height as f32 / f32::from(window_size.cells_height.max(1));
    let area_width_px = f32::from(area.width.max(1)) * cell_width_px;
    let area_height_px = f32::from(area.height.max(1)) * cell_height_px;
    let page_aspect = (page_dimensions.width_pts / page_dimensions.height_pts.max(f32::EPSILON))
        .max(f32::EPSILON);

    let (fit_width_px, fit_height_px) = if area_width_px / area_height_px > page_aspect {
        let height = area_height_px;
        (height * page_aspect, height)
    } else {
        let width = area_width_px;
        (width, width / page_aspect)
    };
    let (render_width_px, render_height_px) =
        ensure_render_floor(fit_width_px.max(1.0), fit_height_px.max(1.0));

    let width_cells = ((fit_width_px / cell_width_px).round() as u16).clamp(1, area.width.max(1));
    let height_cells =
        ((fit_height_px / cell_height_px).round() as u16).clamp(1, area.height.max(1));
    let image_area = Rect {
        x: area.x + (area.width.saturating_sub(width_cells)) / 2,
        y: area.y + (area.height.saturating_sub(height_cells)) / 2,
        width: width_cells,
        height: height_cells,
    };

    FittedPdfPlacement {
        image_area,
        render_width_px,
        render_height_px,
    }
}

fn ensure_render_floor(width_px: f32, height_px: f32) -> (u32, u32) {
    let longest = width_px.max(height_px).max(1.0);
    if longest >= PDF_RENDER_MIN_DIMENSION_PX as f32 {
        return (width_px.round() as u32, height_px.round() as u32);
    }

    let scale = PDF_RENDER_MIN_DIMENSION_PX as f32 / longest;
    (
        (width_px * scale).round().max(1.0) as u32,
        (height_px * scale).round().max(1.0) as u32,
    )
}

fn place_pdf_image(path: &Path, area: Rect) -> Result<()> {
    let place = format!(
        "{}x{}@{}x{}",
        area.width.max(1),
        area.height.max(1),
        area.x,
        area.y
    );
    let status = Command::new("kitten")
        .arg("icat")
        .arg("--stdin=no")
        .arg("--transfer-mode=file")
        .arg("--place")
        .arg(place)
        .arg("--scale-up")
        .arg(path)
        .status()
        .context("failed to start kitten icat")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("kitten icat exited with {status}");
    }
}

fn clear_pdf_images() -> Result<()> {
    let status = Command::new("kitten")
        .arg("icat")
        .arg("--stdin=no")
        .arg("--clear")
        .status()
        .context("failed to start kitten icat")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("kitten icat exited with {status}");
    }
}

fn pdf_render_cache_dir() -> Result<PathBuf> {
    let cache_dir = env::temp_dir().join("elio-pdf-preview");
    fs::create_dir_all(&cache_dir).context("failed to create PDF preview cache")?;
    Ok(cache_dir)
}

fn pdf_render_cache_stem(key: &PdfRenderKey) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("page-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pdfinfo_page_count_reads_page_field() {
        assert_eq!(
            parse_pdfinfo_page_count("Title: demo\nPages: 18\nProducer: test\n"),
            Some(18)
        );
    }

    #[test]
    fn parse_pdfinfo_page_dimensions_reads_global_and_per_page_sizes() {
        assert_eq!(
            parse_pdfinfo_page_dimensions("Page size: 595.276 x 841.89 pts (A4)\n"),
            Some(PdfPageDimensions {
                width_pts: 595.276,
                height_pts: 841.89,
            })
        );
        assert_eq!(
            parse_pdfinfo_page_dimensions("Page    2 size: 300 x 144 pts\n"),
            Some(PdfPageDimensions {
                width_pts: 300.0,
                height_pts: 144.0,
            })
        );
    }

    #[test]
    fn parse_window_size_reads_pixel_dimensions() {
        assert_eq!(parse_window_size("1575x919\n"), Some((1575, 919)));
    }

    #[test]
    fn fit_pdf_page_preserves_aspect_ratio_for_wide_pages() {
        let placement = fit_pdf_page(
            Rect {
                x: 10,
                y: 4,
                width: 30,
                height: 20,
            },
            TerminalWindowSize {
                cells_width: 100,
                cells_height: 50,
                pixels_width: 1000,
                pixels_height: 1000,
            },
            PdfPageDimensions {
                width_pts: 300.0,
                height_pts: 144.0,
            },
        );

        assert!(placement.image_area.width <= 30);
        assert!(placement.image_area.height <= 20);
        assert_eq!(placement.image_area.height, 7);
        assert_eq!(placement.image_area.y, 10);
        assert!(placement.render_width_px > placement.render_height_px);
    }

    #[test]
    fn pdf_preview_page_navigation_clamps_to_document_bounds() {
        let mut app = App::new_at(std::env::temp_dir()).expect("app should initialize");
        app.pdf_preview.enabled = true;
        app.pdf_preview.session = Some(PdfSession {
            path: PathBuf::from("demo.pdf"),
            size: 1,
            modified: None,
            current_page: 2,
            total_pages: Some(3),
        });

        assert!(app.step_pdf_page(1));
        assert_eq!(
            app.pdf_preview
                .session
                .as_ref()
                .map(|session| session.current_page),
            Some(3)
        );
        assert!(!app.step_pdf_page(1));
        assert_eq!(
            app.pdf_preview
                .session
                .as_ref()
                .map(|session| session.current_page),
            Some(3)
        );
        assert!(app.step_pdf_page(-2));
        assert_eq!(
            app.pdf_preview
                .session
                .as_ref()
                .map(|session| session.current_page),
            Some(1)
        );
    }
}
