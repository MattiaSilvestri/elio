use super::*;
use crate::file_facts::{self, DocumentFormat};
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use crossterm::terminal;
use ratatui::layout::Rect;
use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::DefaultHasher},
    env, fs,
    hash::{Hash, Hasher},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};

const PDF_RENDER_CACHE_LIMIT: usize = 12;
const PDF_RENDER_MIN_DIMENSION_PX: u32 = 96;
const PDF_PAGE_MIN: usize = 1;
const PDF_PAGE_STATUS_PREFIX: &str = "PDF page ";
const PDF_SELECTION_ACTIVATION_DELAY: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalImageBackend {
    KittyProtocol,
    Kitten,
}

#[derive(Clone, Debug, Default)]
pub(super) struct PdfPreviewState {
    enabled: bool,
    backend: Option<TerminalImageBackend>,
    session: Option<PdfSession>,
    document_page_counts: HashMap<PdfDocumentKey, usize>,
    page_dimensions: HashMap<PdfPageKey, PdfPageDimensions>,
    pending_page_probes: HashSet<PdfPageKey>,
    failed_page_probes: HashSet<PdfPageKey>,
    rendered_pages: HashMap<PdfRenderKey, PathBuf>,
    render_order: VecDeque<PdfRenderKey>,
    pending_renders: HashSet<PdfRenderKey>,
    failed_renders: HashSet<PdfRenderKey>,
    displayed: Option<DisplayedPdfPreview>,
    terminal_window: Option<TerminalWindowSize>,
    activation_ready_at: Option<Instant>,
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
struct PdfDocumentKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
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
    render_width_px: u32,
    render_height_px: u32,
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PdfProbeResult {
    pub total_pages: Option<usize>,
    pub width_pts: Option<f32>,
    pub height_pts: Option<f32>,
}

impl App {
    pub(crate) fn enable_terminal_pdf_previews(&mut self) {
        self.pdf_preview.backend = detect_terminal_pdf_preview_backend();
        self.pdf_preview.enabled = self.pdf_preview.backend.is_some();
        self.refresh_pdf_terminal_window_size();
        self.sync_pdf_preview_selection();
    }

    pub(crate) fn handle_pdf_terminal_resize(&mut self) {
        self.refresh_pdf_terminal_window_size();
        if self.pdf_preview.session.is_some() {
            self.pdf_preview.activation_ready_at = Some(Instant::now());
            self.queue_current_pdf_render();
        }
    }

    pub(crate) fn present_pdf_overlay(&mut self) -> Result<()> {
        let Some(backend) = self.pdf_preview.backend else {
            self.clear_pdf_overlay()?;
            return Ok(());
        };

        if self
            .pdf_preview
            .displayed
            .as_ref()
            .is_some_and(|displayed| !self.should_keep_displayed_pdf_overlay(displayed))
        {
            self.clear_pdf_overlay()?;
        }

        let Some(request) = self.active_pdf_overlay_request() else {
            self.clear_pdf_overlay()?;
            return Ok(());
        };

        if !self.pdf_selection_activation_ready() {
            return Ok(());
        }

        let Some(placement) = self.overlay_placement_for_request(&request) else {
            let _ = self.ensure_pdf_page_probe(&request);
            return Ok(());
        };
        let displayed = DisplayedPdfPreview::from_request(&request, placement);
        if self.pdf_preview.displayed.as_ref() == Some(&displayed) {
            return Ok(());
        }

        let render_key = PdfRenderKey::from_request(&request, placement);
        let Some(rendered) = self.ensure_pdf_render(&render_key) else {
            return Ok(());
        };

        if self.pdf_preview.displayed.is_some() {
            clear_pdf_images(backend).context("failed to clear previous PDF page")?;
            self.pdf_preview.displayed = None;
        }
        place_pdf_image(backend, &rendered, placement.image_area)
            .context("failed to display PDF page")?;
        self.pdf_preview.displayed = Some(displayed);
        Ok(())
    }

    pub(crate) fn clear_pdf_overlay(&mut self) -> Result<()> {
        if self.pdf_preview.displayed.is_none() {
            return Ok(());
        }
        let Some(backend) = self.pdf_preview.backend else {
            self.pdf_preview.displayed = None;
            return Ok(());
        };
        clear_pdf_images(backend).context("failed to clear PDF preview overlay")?;
        self.pdf_preview.displayed = None;
        Ok(())
    }

    pub(crate) fn preview_uses_image_overlay(&self) -> bool {
        self.active_pdf_display_target()
            .as_ref()
            .zip(self.pdf_preview.displayed.as_ref())
            .is_some_and(|(active, displayed)| active == displayed)
    }

    pub(crate) fn preview_prefers_pdf_surface(&self) -> bool {
        if !self.pdf_preview.enabled
            || self.pdf_preview.backend.is_none()
            || self.pdf_preview.session.is_none()
        {
            return false;
        }
        if self.preview_uses_image_overlay() {
            return true;
        }

        let Some(request) = self.active_pdf_overlay_request() else {
            return false;
        };
        if !self.pdf_selection_activation_ready() {
            return true;
        }

        let page_key = PdfPageKey::from_request(&request);
        if self.pdf_preview.failed_page_probes.contains(&page_key) {
            return false;
        }
        if self.pdf_preview.pending_page_probes.contains(&page_key)
            || !self.pdf_preview.page_dimensions.contains_key(&page_key)
        {
            return true;
        }

        let Some(placement) = self.overlay_placement_for_request(&request) else {
            return false;
        };
        let render_key = PdfRenderKey::from_request(&request, placement);
        if self.pdf_preview.failed_renders.contains(&render_key) {
            return false;
        }
        self.pdf_preview.pending_renders.contains(&render_key)
            || self.cached_render_exists(&render_key)
    }

    pub(crate) fn pdf_preview_placeholder_message(&self) -> Option<String> {
        if !self.preview_prefers_pdf_surface() || self.preview_uses_image_overlay() {
            return None;
        }

        let request = self.active_pdf_overlay_request()?;
        if !self.pdf_selection_activation_ready() {
            return Some("Preparing PDF preview...".to_string());
        }

        let page_key = PdfPageKey::from_request(&request);
        if self.pdf_preview.failed_page_probes.contains(&page_key) {
            return Some("PDF preview unavailable".to_string());
        }
        if !self.pdf_preview.page_dimensions.contains_key(&page_key)
            || self.pdf_preview.pending_page_probes.contains(&page_key)
        {
            return Some("Loading PDF page...".to_string());
        }

        let placement = self.overlay_placement_for_request(&request)?;
        let render_key = PdfRenderKey::from_request(&request, placement);
        if self.pdf_preview.failed_renders.contains(&render_key) {
            return Some("PDF preview unavailable".to_string());
        }
        if self.cached_render_exists(&render_key) {
            return None;
        }
        Some("Rendering PDF page...".to_string())
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
        let changed = session.current_page != previous_page;
        if changed {
            self.pdf_preview.activation_ready_at = Some(Instant::now());
            self.queue_current_pdf_probe();
            self.queue_current_pdf_render();
        }
        changed
    }

    pub(super) fn sync_pdf_preview_selection(&mut self) {
        if !self.pdf_preview.enabled {
            self.pdf_preview.session = None;
            self.pdf_preview.activation_ready_at = None;
            self.clear_pdf_page_status();
            return;
        }

        let Some(entry) = self.selected_entry() else {
            self.pdf_preview.session = None;
            self.pdf_preview.activation_ready_at = None;
            self.clear_pdf_page_status();
            return;
        };
        if !is_pdf_entry(entry) {
            self.pdf_preview.session = None;
            self.pdf_preview.activation_ready_at = None;
            self.clear_pdf_page_status();
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
            total_pages: self.cached_pdf_total_pages(entry),
        });
        self.pdf_preview.activation_ready_at =
            Some(Instant::now() + PDF_SELECTION_ACTIVATION_DELAY);
        self.queue_current_pdf_probe();
        self.queue_current_pdf_render();
        self.clear_pdf_page_status();
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

    fn ensure_pdf_render(&mut self, key: &PdfRenderKey) -> Option<PathBuf> {
        if let Some(path) = self.cached_pdf_render_path(key) {
            return Some(path);
        }
        if self.pdf_preview.failed_renders.contains(key)
            || self.pdf_preview.pending_renders.contains(key)
        {
            return None;
        }
        if !self.scheduler.submit_pdf_render(jobs::PdfRenderRequest {
            path: key.path.clone(),
            size: key.size,
            modified: key.modified,
            page: key.page,
            width_px: key.width_px,
            height_px: key.height_px,
        }) {
            self.pdf_preview.failed_renders.insert(key.clone());
            return None;
        }
        self.pdf_preview.pending_renders.insert(key.clone());
        None
    }

    fn ensure_pdf_page_probe(&mut self, request: &PdfOverlayRequest) -> Option<PdfPageDimensions> {
        let key = PdfPageKey::from_request(request);
        if let Some(dimensions) = self.pdf_preview.page_dimensions.get(&key).copied() {
            return Some(dimensions);
        }
        if self.pdf_preview.failed_page_probes.contains(&key)
            || self.pdf_preview.pending_page_probes.contains(&key)
        {
            return None;
        }
        if !self.scheduler.submit_pdf_probe(jobs::PdfProbeRequest {
            path: request.path.clone(),
            size: request.size,
            modified: request.modified,
            page: request.page,
        }) {
            self.pdf_preview.failed_page_probes.insert(key);
            return None;
        }
        self.pdf_preview.pending_page_probes.insert(key);
        None
    }

    pub(super) fn apply_pdf_probe_build(&mut self, build: jobs::PdfProbeBuild) -> bool {
        let key = PdfPageKey {
            path: build.path.clone(),
            size: build.size,
            modified: build.modified,
            page: build.page,
        };
        self.pdf_preview.pending_page_probes.remove(&key);

        let current_request = self.active_pdf_overlay_request();
        let current_key = current_request.as_ref().map(PdfPageKey::from_request);
        let current_document = self
            .pdf_preview
            .session
            .as_ref()
            .map(PdfDocumentKey::from_session);

        match build.result {
            Ok(result) => {
                self.pdf_preview.failed_page_probes.remove(&key);
                let mut dirty = current_key.as_ref() == Some(&key);
                if let Some(total_pages) = result.total_pages {
                    let document_key = PdfDocumentKey::from_page_key(&key);
                    self.pdf_preview
                        .document_page_counts
                        .insert(document_key.clone(), total_pages);
                    if current_document.as_ref() == Some(&document_key)
                        && let Some(session) = &mut self.pdf_preview.session
                    {
                        let previous_total = session.total_pages;
                        session.total_pages = Some(total_pages);
                        let clamped_page = session
                            .current_page
                            .clamp(PDF_PAGE_MIN, total_pages.max(PDF_PAGE_MIN));
                        if clamped_page != session.current_page {
                            session.current_page = clamped_page;
                            self.pdf_preview.activation_ready_at = Some(Instant::now());
                            dirty = true;
                        }
                        if previous_total != session.total_pages {
                            dirty = true;
                        }
                    }
                }
                if let (Some(width_pts), Some(height_pts)) = (result.width_pts, result.height_pts) {
                    self.pdf_preview.page_dimensions.insert(
                        key.clone(),
                        PdfPageDimensions {
                            width_pts,
                            height_pts,
                        },
                    );
                    dirty |= current_key.as_ref() == Some(&key);
                }
                self.queue_current_pdf_render();
                dirty
            }
            Err(_) => {
                self.pdf_preview.failed_page_probes.insert(key);
                false
            }
        }
    }

    pub(super) fn apply_pdf_render_build(&mut self, build: jobs::PdfRenderBuild) -> bool {
        let key = PdfRenderKey {
            path: build.path.clone(),
            size: build.size,
            modified: build.modified,
            page: build.page,
            width_px: build.width_px,
            height_px: build.height_px,
        };
        self.pdf_preview.pending_renders.remove(&key);

        match build.result {
            Ok(Some(path)) => {
                self.pdf_preview.failed_renders.remove(&key);
                self.remember_rendered_pdf(key.clone(), path);
                self.active_pdf_render_key()
                    .as_ref()
                    .is_some_and(|active| active == &key)
            }
            Ok(None) | Err(_) => {
                self.pdf_preview.failed_renders.insert(key);
                false
            }
        }
    }

    fn clear_pdf_page_status(&mut self) {
        if self.status.starts_with(PDF_PAGE_STATUS_PREFIX) {
            self.status.clear();
        }
    }

    fn refresh_pdf_terminal_window_size(&mut self) {
        self.pdf_preview.terminal_window = if self.pdf_preview.enabled {
            query_terminal_window_size()
        } else {
            None
        };
    }

    fn cached_pdf_total_pages(&self, entry: &Entry) -> Option<usize> {
        self.pdf_preview
            .document_page_counts
            .get(&PdfDocumentKey::from_entry(entry))
            .copied()
    }

    fn pdf_selection_activation_ready(&self) -> bool {
        self.pdf_preview
            .activation_ready_at
            .is_none_or(|ready_at| Instant::now() >= ready_at)
    }

    fn overlay_placement_for_request(
        &self,
        request: &PdfOverlayRequest,
    ) -> Option<FittedPdfPlacement> {
        let window_size = self.cached_pdf_terminal_window()?;
        let page_dimensions = self.cached_pdf_page_dimensions(request)?;
        Some(fit_pdf_page(request.area, window_size, page_dimensions))
    }

    fn cached_pdf_terminal_window(&self) -> Option<TerminalWindowSize> {
        let cached = self.pdf_preview.terminal_window?;
        let Ok((cells_width, cells_height)) = terminal::size() else {
            return Some(cached);
        };
        if cached.cells_width == cells_width && cached.cells_height == cells_height {
            Some(cached)
        } else {
            None
        }
    }

    fn cached_pdf_page_dimensions(&self, request: &PdfOverlayRequest) -> Option<PdfPageDimensions> {
        self.pdf_preview
            .page_dimensions
            .get(&PdfPageKey::from_request(request))
            .copied()
    }

    fn cached_pdf_render_path(&mut self, key: &PdfRenderKey) -> Option<PathBuf> {
        if let Some(path) = self.pdf_preview.rendered_pages.get(key)
            && path.exists()
        {
            return Some(path.clone());
        }

        self.pdf_preview.rendered_pages.remove(key);
        self.pdf_preview.render_order.retain(|queued| queued != key);
        None
    }

    fn cached_render_exists(&self, key: &PdfRenderKey) -> bool {
        self.pdf_preview
            .rendered_pages
            .get(key)
            .is_some_and(|path| path.exists())
    }

    fn remember_rendered_pdf(&mut self, key: PdfRenderKey, path: PathBuf) {
        self.pdf_preview.rendered_pages.insert(key.clone(), path);
        self.pdf_preview
            .render_order
            .retain(|queued| queued != &key);
        self.pdf_preview.render_order.push_back(key);
        while self.pdf_preview.render_order.len() > PDF_RENDER_CACHE_LIMIT {
            if let Some(stale_key) = self.pdf_preview.render_order.pop_front()
                && let Some(stale_path) = self.pdf_preview.rendered_pages.remove(&stale_key)
            {
                let _ = fs::remove_file(stale_path);
            }
        }
    }

    fn active_pdf_display_target(&self) -> Option<DisplayedPdfPreview> {
        let request = self.active_pdf_overlay_request()?;
        if !self.pdf_selection_activation_ready() {
            return None;
        }
        let placement = self.overlay_placement_for_request(&request)?;
        Some(DisplayedPdfPreview::from_request(&request, placement))
    }

    fn active_pdf_render_key(&self) -> Option<PdfRenderKey> {
        let request = self.active_pdf_overlay_request()?;
        if !self.pdf_selection_activation_ready() {
            return None;
        }
        let placement = self.overlay_placement_for_request(&request)?;
        Some(PdfRenderKey::from_request(&request, placement))
    }

    fn should_keep_displayed_pdf_overlay(&self, displayed: &DisplayedPdfPreview) -> bool {
        self.active_pdf_display_target()
            .as_ref()
            .is_some_and(|target| target == displayed)
    }

    fn queue_current_pdf_probe(&mut self) {
        let Some(session) = &self.pdf_preview.session else {
            return;
        };

        let key = PdfPageKey {
            path: session.path.clone(),
            size: session.size,
            modified: session.modified,
            page: session.current_page,
        };
        if self.pdf_preview.page_dimensions.contains_key(&key)
            || self.pdf_preview.pending_page_probes.contains(&key)
            || self.pdf_preview.failed_page_probes.contains(&key)
        {
            return;
        }

        if self.scheduler.submit_pdf_probe(jobs::PdfProbeRequest {
            path: key.path.clone(),
            size: key.size,
            modified: key.modified,
            page: key.page,
        }) {
            self.pdf_preview.pending_page_probes.insert(key);
        } else {
            self.pdf_preview.failed_page_probes.insert(key);
        }
    }

    fn queue_current_pdf_render(&mut self) {
        let Some(request) = self.active_pdf_overlay_request() else {
            return;
        };
        let Some(placement) = self.overlay_placement_for_request(&request) else {
            return;
        };
        let key = PdfRenderKey::from_request(&request, placement);
        if self.cached_render_exists(&key)
            || self.pdf_preview.pending_renders.contains(&key)
            || self.pdf_preview.failed_renders.contains(&key)
        {
            return;
        }

        if self.scheduler.submit_pdf_render(jobs::PdfRenderRequest {
            path: key.path.clone(),
            size: key.size,
            modified: key.modified,
            page: key.page,
            width_px: key.width_px,
            height_px: key.height_px,
        }) {
            self.pdf_preview.pending_renders.insert(key);
        } else {
            self.pdf_preview.failed_renders.insert(key);
        }
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
    fn from_request(request: &PdfOverlayRequest, placement: FittedPdfPlacement) -> Self {
        Self {
            path: request.path.clone(),
            size: request.size,
            modified: request.modified,
            page: request.page,
            area: request.area,
            render_width_px: placement.render_width_px,
            render_height_px: placement.render_height_px,
        }
    }
}

impl PdfDocumentKey {
    fn from_entry(entry: &Entry) -> Self {
        Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
        }
    }

    fn from_page_key(key: &PdfPageKey) -> Self {
        Self {
            path: key.path.clone(),
            size: key.size,
            modified: key.modified,
        }
    }

    fn from_session(session: &PdfSession) -> Self {
        Self {
            path: session.path.clone(),
            size: session.size,
            modified: session.modified,
        }
    }
}

fn detect_terminal_pdf_preview_backend() -> Option<TerminalImageBackend> {
    if !command_exists("pdftocairo") {
        return None;
    }

    let term = env::var("TERM").unwrap_or_default();
    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();
    let kitten_available = command_exists("kitten");
    let kitten_detected = kitten_available && detect_kitten_backend_support();

    select_terminal_image_backend(
        &term,
        &term_program,
        env::var_os("KITTY_WINDOW_ID").is_some(),
        kitten_available,
        kitten_detected,
    )
}

fn detect_kitten_backend_support() -> bool {
    Command::new("kitten")
        .arg("icat")
        .arg("--stdin=no")
        .arg("--detect-support")
        .arg("--detection-timeout=1")
        .status()
        .is_ok_and(|status| status.success())
}

fn select_terminal_image_backend(
    term: &str,
    term_program: &str,
    kitty_window_id_present: bool,
    kitten_available: bool,
    kitten_detected: bool,
) -> Option<TerminalImageBackend> {
    let term = term.to_ascii_lowercase();
    let term_program = term_program.to_ascii_lowercase();
    let supports_kitty_protocol = kitty_window_id_present
        || term.contains("xterm-kitty")
        || term.contains("ghostty")
        || term.contains("wezterm")
        || matches!(term_program.as_str(), "kitty" | "ghostty" | "wezterm");

    if supports_kitty_protocol {
        Some(TerminalImageBackend::KittyProtocol)
    } else if kitten_available && kitten_detected {
        Some(TerminalImageBackend::Kitten)
    } else {
        None
    }
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

pub(super) fn probe_pdf_page(path: &Path, page: usize) -> Result<PdfProbeResult> {
    let output = Command::new("pdfinfo")
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg(path)
        .output()
        .context("failed to start pdfinfo")?;
    if !output.status.success() {
        anyhow::bail!("pdfinfo exited with {}", output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let dimensions = parse_pdfinfo_page_dimensions(&stdout);
    Ok(PdfProbeResult {
        total_pages: parse_pdfinfo_page_count(&stdout),
        width_pts: dimensions.map(|dimensions| dimensions.width_pts),
        height_pts: dimensions.map(|dimensions| dimensions.height_pts),
    })
}

pub(super) fn render_pdf_page_to_cache(
    path: &Path,
    size: u64,
    modified: Option<SystemTime>,
    page: usize,
    width_px: u32,
    height_px: u32,
) -> Result<Option<PathBuf>> {
    let key = PdfRenderKey {
        path: path.to_path_buf(),
        size,
        modified,
        page,
        width_px,
        height_px,
    };
    let cache_dir = pdf_render_cache_dir()?;
    let stem = cache_dir.join(pdf_render_cache_stem(&key));
    let png_path = stem.with_extension("png");
    if png_path.exists() {
        return Ok(Some(png_path));
    }

    let status = Command::new("pdftocairo")
        .arg("-png")
        .arg("-singlefile")
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg("-scale-to-x")
        .arg(width_px.to_string())
        .arg("-scale-to-y")
        .arg("-1")
        .arg(path)
        .arg(&stem)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to start pdftocairo")?;

    if !status.success() || !png_path.exists() {
        return Ok(None);
    }
    Ok(Some(png_path))
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
    let terminal_size = terminal::window_size().ok();
    let (cells_width, cells_height) = terminal_size
        .as_ref()
        .map(|size| (size.columns, size.rows))
        .or_else(|| terminal::size().ok())?;
    let (pixels_width, pixels_height) = terminal_size
        .as_ref()
        .and_then(|size| {
            let width = u32::from(size.width);
            let height = u32::from(size.height);
            (width > 0 && height > 0).then_some((width, height))
        })
        .or_else(query_kitten_window_size)
        .unwrap_or_else(|| fallback_window_size_pixels(cells_width, cells_height));
    Some(TerminalWindowSize {
        cells_width,
        cells_height,
        pixels_width,
        pixels_height,
    })
}

fn query_kitten_window_size() -> Option<(u32, u32)> {
    if !command_exists("kitten") {
        return None;
    }

    let output = Command::new("kitten")
        .arg("icat")
        .arg("--print-window-size")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_window_size(&String::from_utf8_lossy(&output.stdout))
}

fn parse_window_size(output: &str) -> Option<(u32, u32)> {
    let trimmed = output.trim();
    let (width, height) = trimmed.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn fallback_window_size_pixels(cells_width: u16, cells_height: u16) -> (u32, u32) {
    (
        u32::from(cells_width.max(1)) * 8,
        u32::from(cells_height.max(1)) * 16,
    )
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

fn place_pdf_image(backend: TerminalImageBackend, path: &Path, area: Rect) -> Result<()> {
    match backend {
        TerminalImageBackend::Kitten => place_pdf_image_with_kitten(path, area),
        TerminalImageBackend::KittyProtocol => place_pdf_image_with_kitty_protocol(path, area),
    }
}

fn place_pdf_image_with_kitten(path: &Path, area: Rect) -> Result<()> {
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

fn place_pdf_image_with_kitty_protocol(path: &Path, area: Rect) -> Result<()> {
    write_terminal_escape(&build_kitty_display_sequence(path, area))
}

fn clear_pdf_images(backend: TerminalImageBackend) -> Result<()> {
    match backend {
        TerminalImageBackend::Kitten => clear_pdf_images_with_kitten(),
        TerminalImageBackend::KittyProtocol => clear_pdf_images_with_kitty_protocol(),
    }
}

fn clear_pdf_images_with_kitten() -> Result<()> {
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

fn clear_pdf_images_with_kitty_protocol() -> Result<()> {
    write_terminal_escape(build_kitty_clear_sequence())
}

fn write_terminal_escape(sequence: &str) -> Result<()> {
    let mut stdout = io::stdout();
    stdout
        .write_all(sequence.as_bytes())
        .context("failed to write terminal escape")?;
    stdout.flush().context("failed to flush terminal escape")?;
    Ok(())
}

fn build_kitty_display_sequence(path: &Path, area: Rect) -> String {
    let payload = BASE64_STANDARD.encode(path.as_os_str().as_encoded_bytes());
    format!(
        "\u{1b}[{};{}H\u{1b}_Ga=T,q=2,f=100,t=f,c={},r={},C=1;{}\u{1b}\\",
        area.y.saturating_add(1),
        area.x.saturating_add(1),
        area.width.max(1),
        area.height.max(1),
        payload
    )
}

fn build_kitty_clear_sequence() -> &'static str {
    "\u{1b}_Ga=d,d=A,q=2\u{1b}\\"
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
    use std::{
        fs,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("elio-pdf-preview-{label}-{unique}"))
    }

    fn build_pdf_overlay_test_app(label: &str) -> (App, PathBuf) {
        let root = temp_root(label);
        fs::create_dir_all(&root).expect("failed to create temp root");

        let mut app = App::new_at(root.clone()).expect("app should initialize");
        let (cells_width, cells_height) = terminal::size().unwrap_or((120, 40));
        app.pdf_preview.enabled = true;
        app.pdf_preview.backend = Some(TerminalImageBackend::KittyProtocol);
        app.pdf_preview.session = Some(PdfSession {
            path: root.join("demo.pdf"),
            size: 128,
            modified: None,
            current_page: 1,
            total_pages: None,
        });
        app.frame_state.preview_content_area = Some(Rect {
            x: 2,
            y: 3,
            width: 48,
            height: 20,
        });
        app.pdf_preview.terminal_window = Some(TerminalWindowSize {
            cells_width,
            cells_height,
            pixels_width: 1920,
            pixels_height: 1080,
        });
        app.pdf_preview.activation_ready_at = Some(Instant::now());
        (app, root)
    }

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
    fn select_terminal_image_backend_prefers_known_kitty_protocol_terminals() {
        assert_eq!(
            select_terminal_image_backend("xterm-kitty", "", false, false, false),
            Some(TerminalImageBackend::KittyProtocol)
        );
        assert_eq!(
            select_terminal_image_backend("xterm-256color", "ghostty", false, false, false),
            Some(TerminalImageBackend::KittyProtocol)
        );
        assert_eq!(
            select_terminal_image_backend("xterm-256color", "WezTerm", false, false, false),
            Some(TerminalImageBackend::KittyProtocol)
        );
        assert_eq!(
            select_terminal_image_backend("screen-256color", "", true, false, false),
            Some(TerminalImageBackend::KittyProtocol)
        );
    }

    #[test]
    fn select_terminal_image_backend_falls_back_to_kitten_detection() {
        assert_eq!(
            select_terminal_image_backend("xterm-256color", "", false, true, true),
            Some(TerminalImageBackend::Kitten)
        );
        assert_eq!(
            select_terminal_image_backend("xterm-256color", "", false, true, false),
            None
        );
    }

    #[test]
    fn fallback_window_size_pixels_uses_reasonable_cell_defaults() {
        assert_eq!(fallback_window_size_pixels(100, 40), (800, 640));
        assert_eq!(fallback_window_size_pixels(0, 0), (8, 16));
    }

    #[test]
    fn build_kitty_display_sequence_positions_png_without_cursor_motion() {
        let path = Path::new("/tmp/demo.pdf-preview.png");
        let area = Rect {
            x: 7,
            y: 4,
            width: 30,
            height: 12,
        };

        let sequence = build_kitty_display_sequence(path, area);

        assert!(sequence.starts_with("\u{1b}[5;8H\u{1b}_G"));
        assert!(sequence.contains("a=T"));
        assert!(sequence.contains("q=2"));
        assert!(sequence.contains("f=100"));
        assert!(sequence.contains("t=f"));
        assert!(sequence.contains("c=30"));
        assert!(sequence.contains("r=12"));
        assert!(sequence.contains("C=1"));
        assert!(sequence.contains(&BASE64_STANDARD.encode(path.as_os_str().as_encoded_bytes())));
        assert!(sequence.ends_with("\u{1b}\\"));
    }

    #[test]
    fn build_kitty_clear_sequence_deletes_visible_images() {
        assert_eq!(build_kitty_clear_sequence(), "\u{1b}_Ga=d,d=A,q=2\u{1b}\\");
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
        app.pdf_preview.backend = Some(TerminalImageBackend::KittyProtocol);
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
        assert!(app.status.is_empty());
    }

    #[test]
    fn present_pdf_overlay_waits_for_selection_activation_before_queueing_probe() {
        let (mut app, root) = build_pdf_overlay_test_app("activation-delay");
        app.pdf_preview.activation_ready_at = Some(Instant::now() + Duration::from_secs(5));

        app.present_pdf_overlay()
            .expect("presenting a delayed PDF overlay should not fail");

        assert!(app.pdf_preview.pending_page_probes.is_empty());
        assert!(!app.scheduler.has_pending_work());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn present_pdf_overlay_queues_current_probe_only_once() {
        let (mut app, root) = build_pdf_overlay_test_app("probe-queue");
        let request = app
            .active_pdf_overlay_request()
            .expect("PDF overlay request should be available");
        let key = PdfPageKey::from_request(&request);

        app.present_pdf_overlay()
            .expect("presenting a PDF overlay should not fail");
        app.present_pdf_overlay()
            .expect("retrying a PDF overlay should not fail");

        assert_eq!(app.pdf_preview.pending_page_probes.len(), 1);
        assert!(app.pdf_preview.pending_page_probes.contains(&key));
        assert!(app.scheduler.has_pending_work());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn sync_pdf_preview_selection_reuses_cached_total_page_count() {
        let root = temp_root("cached-page-count");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let mut app = App::new_at(root.clone()).expect("app should initialize");
        let entry = Entry {
            path: root.join("cached.pdf"),
            name: "cached.pdf".to_string(),
            name_key: "cached.pdf".to_string(),
            kind: EntryKind::File,
            size: 64,
            modified: None,
            readonly: false,
        };
        app.entries = vec![entry.clone()];
        app.selected = 0;
        app.pdf_preview.enabled = true;
        app.pdf_preview.backend = Some(TerminalImageBackend::KittyProtocol);
        app.pdf_preview
            .document_page_counts
            .insert(PdfDocumentKey::from_entry(&entry), 12);

        app.sync_pdf_preview_selection();

        assert_eq!(
            app.pdf_preview
                .session
                .as_ref()
                .and_then(|session| session.total_pages),
            Some(12)
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn sync_pdf_preview_selection_queues_initial_probe_for_current_page() {
        let root = temp_root("selection-probe");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let mut app = App::new_at(root.clone()).expect("app should initialize");
        let entry = Entry {
            path: root.join("queued.pdf"),
            name: "queued.pdf".to_string(),
            name_key: "queued.pdf".to_string(),
            kind: EntryKind::File,
            size: 64,
            modified: None,
            readonly: false,
        };
        app.entries = vec![entry.clone()];
        app.selected = 0;
        app.pdf_preview.enabled = true;
        app.pdf_preview.backend = Some(TerminalImageBackend::KittyProtocol);

        app.sync_pdf_preview_selection();

        assert!(app.scheduler.has_pending_work());
        assert!(app.pdf_preview.pending_page_probes.contains(&PdfPageKey {
            path: entry.path,
            size: entry.size,
            modified: entry.modified,
            page: PDF_PAGE_MIN,
        }));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn apply_pdf_probe_build_updates_current_session_and_cached_dimensions() {
        let (mut app, root) = build_pdf_overlay_test_app("probe-apply");
        let session = app
            .pdf_preview
            .session
            .as_mut()
            .expect("PDF session should exist");
        session.current_page = 5;
        let key = PdfPageKey {
            path: root.join("demo.pdf"),
            size: 128,
            modified: None,
            page: 5,
        };
        app.pdf_preview.pending_page_probes.insert(key.clone());

        let dirty = app.apply_pdf_probe_build(jobs::PdfProbeBuild {
            path: root.join("demo.pdf"),
            size: 128,
            modified: None,
            page: 5,
            result: Ok(PdfProbeResult {
                total_pages: Some(3),
                width_pts: Some(300.0),
                height_pts: Some(144.0),
            }),
        });

        assert!(dirty);
        assert_eq!(
            app.pdf_preview
                .session
                .as_ref()
                .map(|session| session.current_page),
            Some(3)
        );
        assert_eq!(
            app.pdf_preview
                .session
                .as_ref()
                .and_then(|session| session.total_pages),
            Some(3)
        );
        assert_eq!(
            app.pdf_preview.page_dimensions.get(&key),
            Some(&PdfPageDimensions {
                width_pts: 300.0,
                height_pts: 144.0,
            })
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn apply_pdf_probe_build_queues_render_for_current_page() {
        let (mut app, root) = build_pdf_overlay_test_app("probe-render-queue");
        let request = app
            .active_pdf_overlay_request()
            .expect("PDF overlay request should be available");
        let page_key = PdfPageKey::from_request(&request);
        app.pdf_preview.pending_page_probes.insert(page_key);

        let dirty = app.apply_pdf_probe_build(jobs::PdfProbeBuild {
            path: request.path.clone(),
            size: request.size,
            modified: request.modified,
            page: request.page,
            result: Ok(PdfProbeResult {
                total_pages: Some(8),
                width_pts: Some(595.0),
                height_pts: Some(842.0),
            }),
        });

        let placement = app
            .overlay_placement_for_request(&request)
            .expect("overlay placement should be available after probe");
        let render_key = PdfRenderKey::from_request(&request, placement);

        assert!(dirty);
        assert!(app.pdf_preview.pending_renders.contains(&render_key));
        assert!(app.scheduler.has_pending_work());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_uses_image_overlay_only_for_current_render_target() {
        let (mut app, root) = build_pdf_overlay_test_app("overlay-match");
        let request = app
            .active_pdf_overlay_request()
            .expect("PDF overlay request should be available");
        let key = PdfPageKey::from_request(&request);
        app.pdf_preview.page_dimensions.insert(
            key,
            PdfPageDimensions {
                width_pts: 595.0,
                height_pts: 842.0,
            },
        );
        let placement = app
            .overlay_placement_for_request(&request)
            .expect("overlay placement should be available");
        app.pdf_preview.displayed = Some(DisplayedPdfPreview::from_request(&request, placement));

        assert!(app.preview_uses_image_overlay());

        app.pdf_preview
            .session
            .as_mut()
            .expect("PDF session should exist")
            .current_page = 2;

        assert!(!app.preview_uses_image_overlay());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn step_pdf_page_queues_render_immediately_when_dimensions_are_cached() {
        let (mut app, root) = build_pdf_overlay_test_app("page-step-render");
        let next_request = PdfOverlayRequest {
            path: root.join("demo.pdf"),
            size: 128,
            modified: None,
            page: 2,
            area: app
                .frame_state
                .preview_content_area
                .expect("preview content area should be set"),
        };
        app.pdf_preview.page_dimensions.insert(
            PdfPageKey::from_request(&next_request),
            PdfPageDimensions {
                width_pts: 612.0,
                height_pts: 792.0,
            },
        );
        app.pdf_preview
            .session
            .as_mut()
            .expect("PDF session should exist")
            .total_pages = Some(3);

        assert!(app.step_pdf_page(1));

        let active_request = app
            .active_pdf_overlay_request()
            .expect("updated PDF overlay request should be available");
        let placement = app
            .overlay_placement_for_request(&active_request)
            .expect("overlay placement should be available");
        let render_key = PdfRenderKey::from_request(&active_request, placement);
        assert!(app.pdf_preview.pending_renders.contains(&render_key));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn pdf_preview_placeholder_message_tracks_loading_state() {
        let (mut app, root) = build_pdf_overlay_test_app("placeholder");

        assert_eq!(
            app.pdf_preview_placeholder_message().as_deref(),
            Some("Loading PDF page...")
        );

        let request = app
            .active_pdf_overlay_request()
            .expect("PDF overlay request should be available");
        let page_key = PdfPageKey::from_request(&request);
        app.pdf_preview.page_dimensions.insert(
            page_key,
            PdfPageDimensions {
                width_pts: 595.0,
                height_pts: 842.0,
            },
        );
        let placement = app
            .overlay_placement_for_request(&request)
            .expect("overlay placement should be available");
        app.pdf_preview
            .pending_renders
            .insert(PdfRenderKey::from_request(&request, placement));

        assert_eq!(
            app.pdf_preview_placeholder_message().as_deref(),
            Some("Rendering PDF page...")
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn preview_prefers_pdf_surface_falls_back_after_overlay_failure() {
        let (mut app, root) = build_pdf_overlay_test_app("fallback");
        let request = app
            .active_pdf_overlay_request()
            .expect("PDF overlay request should be available");
        let page_key = PdfPageKey::from_request(&request);
        app.pdf_preview.failed_page_probes.insert(page_key);

        assert!(!app.preview_prefers_pdf_surface());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn sync_pdf_preview_selection_clears_stale_pdf_page_status() {
        let mut app = App::new_at(std::env::temp_dir()).expect("app should initialize");
        app.status = "PDF page 3/10".to_string();
        app.pdf_preview.enabled = true;
        app.pdf_preview.backend = Some(TerminalImageBackend::KittyProtocol);

        app.sync_pdf_preview_selection();

        assert!(app.status.is_empty());
    }
}
