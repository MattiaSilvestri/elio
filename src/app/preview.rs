mod fallback;
mod markdown;
mod syntax;

use super::*;
use crate::appearance;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    process::Command,
};

const PREVIEW_LIMIT_BYTES: usize = 64 * 1024;
const PREVIEW_RENDER_LINE_LIMIT: usize = 240;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PreviewKind {
    Directory,
    Markdown,
    Code,
    Text,
    Binary,
    Unavailable,
}

impl PreviewKind {
    pub(super) fn section_label(self) -> &'static str {
        match self {
            Self::Directory => "Contents",
            Self::Markdown => "Markdown",
            Self::Code => "Code",
            Self::Text => "Text",
            Self::Binary | Self::Unavailable => "Preview",
        }
    }

    pub(super) fn wraps_in_preview(self) -> bool {
        matches!(
            self,
            Self::Markdown | Self::Text | Self::Binary | Self::Unavailable
        )
    }

    pub(super) fn allows_horizontal_scroll(self) -> bool {
        self == Self::Code
    }
}

#[derive(Clone, Debug)]
pub(super) struct PreviewContent {
    pub kind: PreviewKind,
    pub detail: Option<String>,
    pub truncated: bool,
    pub truncation_note: Option<String>,
    pub source_lines: Option<usize>,
    pub item_count: Option<usize>,
    pub folder_count: Option<usize>,
    pub file_count: Option<usize>,
    pub lines: Vec<Line<'static>>,
}

struct TextPreview {
    text: String,
    bytes_truncated: bool,
}

#[derive(Default)]
struct TorrentMetadata {
    name: Option<String>,
    announce: Option<String>,
    comment: Option<String>,
    created_by: Option<String>,
    file_count: usize,
    total_size: Option<u64>,
}

impl PreviewContent {
    pub(super) fn new(kind: PreviewKind, lines: Vec<Line<'static>>) -> Self {
        Self {
            kind,
            detail: None,
            truncated: false,
            truncation_note: None,
            source_lines: None,
            item_count: None,
            folder_count: None,
            file_count: None,
            lines,
        }
    }

    pub(super) fn placeholder(label: &str) -> Self {
        Self::new(
            PreviewKind::Unavailable,
            vec![Line::from(label.to_string())],
        )
    }

    pub(super) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(super) fn with_source_lines(mut self, source_lines: usize) -> Self {
        self.source_lines = Some(source_lines.max(1));
        self
    }

    pub(super) fn with_truncation(mut self, note: impl Into<String>) -> Self {
        self.truncated = true;
        self.truncation_note = Some(note.into());
        self
    }

    pub(super) fn with_directory_counts(
        mut self,
        item_count: usize,
        folder_count: usize,
        file_count: usize,
    ) -> Self {
        self.item_count = Some(item_count);
        self.folder_count = Some(folder_count);
        self.file_count = Some(file_count);
        self
    }

    pub(super) fn section_label(&self) -> &'static str {
        self.kind.section_label()
    }

    pub(super) fn total_lines(&self) -> usize {
        self.lines.len()
    }

    pub(super) fn lines(&self) -> Vec<Line<'static>> {
        self.lines.clone()
    }

    pub(super) fn visual_line_count(&self, width: usize) -> usize {
        if !self.kind.wraps_in_preview() {
            return self.total_lines();
        }
        let width = width.max(1);
        self.lines
            .iter()
            .map(|line| {
                let line_width = preview_line_width(line);
                line_width.max(1).div_ceil(width)
            })
            .sum::<usize>()
            .max(1)
    }

    pub(super) fn max_line_width(&self) -> usize {
        self.lines.iter().map(preview_line_width).max().unwrap_or(0)
    }

    pub(super) fn header_detail(&self, offset: usize, visible_rows: usize) -> Option<String> {
        if self.kind == PreviewKind::Directory {
            return None;
        }

        let mut parts = Vec::new();
        if let Some(detail) = &self.detail
            && !detail.is_empty()
        {
            parts.push(detail.clone());
        }

        if let Some(source_lines) = self.source_lines {
            parts.push(format!("{source_lines} lines"));
        }

        if let Some(note) = &self.truncation_note {
            parts.push(note.clone());
        }

        if !parts.is_empty() {
            return Some(parts.join("  •  "));
        }

        let rendered_total = self.total_lines();
        if rendered_total == 0 {
            return self.detail.clone();
        }

        let start = offset.saturating_add(1);
        let end = (offset + visible_rows.max(1)).min(rendered_total);
        let range = if rendered_total > visible_rows.max(1) {
            format!("{start}-{end} / {rendered_total}")
        } else {
            format!("{rendered_total} lines")
        };

        match &self.detail {
            Some(detail) if !detail.is_empty() => Some(format!("{detail}  •  {range}")),
            _ => Some(range),
        }
    }
}

fn preview_line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn status_preview(
    kind: PreviewKind,
    detail: impl Into<String>,
    lines: impl IntoIterator<Item = Line<'static>>,
) -> PreviewContent {
    PreviewContent::new(kind, lines.into_iter().collect()).with_detail(detail)
}

pub(super) fn build_preview(entry: &Entry) -> PreviewContent {
    if entry.is_dir() {
        return build_directory_preview(entry);
    }

    let type_detail = appearance::specific_type_label(&entry.path, entry.kind);
    if is_iso_path(&entry.path)
        && let Some(preview) = build_iso_preview(&entry.path)
    {
        return preview;
    }
    if is_torrent_path(&entry.path)
        && let Some(preview) = build_torrent_preview(&entry.path)
    {
        return preview;
    }

    let text_preview = match read_text_preview(&entry.path) {
        Ok(Some(text)) => text,
        Ok(None) => return apply_type_detail(binary_preview(), type_detail),
        Err(_) => {
            return apply_type_detail(
                unavailable_preview("The file could not be read"),
                type_detail,
            );
        }
    };
    let source_line_count = count_source_lines(&text_preview.text);
    let line_truncated = source_line_count > PREVIEW_RENDER_LINE_LIMIT;
    let truncation_note = truncation_note(text_preview.bytes_truncated, line_truncated);
    let syntax_hint = syntax::preview_syntax_hint(&entry.path);
    let fallback_syntax = fallback::preview_fallback_syntax(&entry.path);

    if is_markdown_path(&entry.path) {
        let preview = PreviewContent::new(
            PreviewKind::Markdown,
            markdown::render_markdown_preview(&text_preview.text),
        );
        return finalize_text_preview(
            preview,
            source_line_count,
            text_preview.bytes_truncated,
            truncation_note,
        );
    }

    if let Some(fallback_syntax) = fallback_syntax
        && matches!(fallback_syntax, fallback::FallbackSyntax::DesktopEntry)
    {
        let preview = PreviewContent::new(
            PreviewKind::Code,
            fallback::render_fallback_code_preview(&text_preview.text, fallback_syntax, true),
        )
        .with_detail(fallback_syntax.detail_label());
        return finalize_text_preview(
            preview,
            source_line_count,
            text_preview.bytes_truncated,
            truncation_note.clone(),
        );
    }

    if let Some(syntax) = syntax::preview_code_syntax(entry, syntax::syntax_set(), syntax_hint) {
        let preview = PreviewContent::new(
            PreviewKind::Code,
            syntax::render_code_preview(&entry.path, &text_preview.text, syntax_hint, true),
        )
        .with_detail(syntax.name.clone());
        return finalize_text_preview(
            preview,
            source_line_count,
            text_preview.bytes_truncated,
            truncation_note,
        );
    }

    if let Some(fallback_syntax) = fallback_syntax {
        let preview = PreviewContent::new(
            PreviewKind::Code,
            fallback::render_fallback_code_preview(&text_preview.text, fallback_syntax, true),
        )
        .with_detail(fallback_syntax.detail_label());
        return finalize_text_preview(
            preview,
            source_line_count,
            text_preview.bytes_truncated,
            truncation_note,
        );
    }

    let preview = PreviewContent::new(
        PreviewKind::Text,
        render_plain_text_preview(&text_preview.text),
    );
    finalize_text_preview(
        apply_type_detail(preview, type_detail),
        source_line_count,
        text_preview.bytes_truncated,
        truncation_note,
    )
}

fn build_directory_preview(entry: &Entry) -> PreviewContent {
    match fs::read_dir(&entry.path) {
        Ok(children) => {
            let mut items = children
                .flatten()
                .map(|child| {
                    let path = child.path();
                    let file_name = child.file_name().to_string_lossy().to_string();
                    let is_dir = path.is_dir();
                    (file_name, path, is_dir)
                })
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .2
                    .cmp(&left.2)
                    .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
            });

            if items.is_empty() {
                return status_preview(
                    PreviewKind::Directory,
                    "0 items",
                    [Line::from("Folder is empty")],
                );
            }

            let palette = appearance::palette();
            let total_items = items.len();
            let folder_count = items.iter().filter(|item| item.2).count();
            let file_count = total_items.saturating_sub(folder_count);
            let mut lines = Vec::new();
            for (name, path, is_dir) in items.into_iter().take(PREVIEW_RENDER_LINE_LIMIT) {
                let appearance = appearance::resolve_path(
                    &path,
                    if is_dir {
                        EntryKind::Directory
                    } else {
                        EntryKind::File
                    },
                );
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", appearance.icon),
                        Style::default()
                            .fg(appearance.color)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::styled(name, Style::default().fg(palette.text)),
                ]));
            }

            PreviewContent::new(PreviewKind::Directory, lines)
                .with_detail(format!("{total_items} items"))
                .with_directory_counts(total_items, folder_count, file_count)
        }
        Err(_) => unavailable_preview("Folder preview unavailable"),
    }
}

fn build_iso_preview(path: &Path) -> Option<PreviewContent> {
    let output = Command::new("bsdtar").arg("-tf").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let listing = String::from_utf8(output.stdout).ok()?;
    let items = listing
        .lines()
        .map(trim_trailing_line_endings)
        .map(|line| line.trim().trim_start_matches("./").to_string())
        .filter(|line| !line.is_empty() && line != "." && line != "./")
        .collect::<Vec<_>>();

    Some(build_listing_preview(&items, "Image is empty"))
}

fn build_torrent_preview(path: &Path) -> Option<PreviewContent> {
    const TORRENT_PREVIEW_LIMIT_BYTES: u64 = 1024 * 1024;

    let mut bytes = Vec::with_capacity(TORRENT_PREVIEW_LIMIT_BYTES as usize);
    File::open(path)
        .ok()?
        .take(TORRENT_PREVIEW_LIMIT_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;

    let mut index = 0usize;
    let mut metadata = TorrentMetadata::default();
    parse_torrent_root(&bytes, &mut index, &mut metadata)?;

    let mut lines = Vec::new();
    push_preview_line(
        &mut lines,
        "Name",
        metadata.name.unwrap_or_else(|| "unknown".to_string()),
    );
    if let Some(tracker) = metadata.announce {
        push_preview_line(&mut lines, "Tracker", tracker);
    }
    let files = if metadata.file_count > 0 {
        metadata.file_count
    } else {
        1
    };
    push_preview_line(&mut lines, "Files", files.to_string());
    if let Some(total_size) = metadata.total_size {
        push_preview_line(&mut lines, "Size", crate::app::format_size(total_size));
    }
    if let Some(created_by) = metadata.created_by {
        push_preview_line(&mut lines, "Created", created_by);
    }
    if let Some(comment) = metadata.comment {
        push_preview_line(&mut lines, "Comment", comment);
    }

    let file_count = files;
    Some(
        PreviewContent::new(PreviewKind::Text, lines)
            .with_detail("BitTorrent file")
            .with_directory_counts(file_count, 0, file_count),
    )
}

fn build_listing_preview(items: &[String], empty_label: &str) -> PreviewContent {
    if items.is_empty() {
        return status_preview(
            PreviewKind::Directory,
            "0 items",
            [Line::from(empty_label.to_string())],
        );
    }

    let palette = appearance::palette();
    let total_items = items.len();
    let folder_count = items
        .iter()
        .filter(|item| item.ends_with('/') || item.ends_with('\\'))
        .count();
    let file_count = total_items.saturating_sub(folder_count);
    let mut lines = Vec::new();

    for item in items.iter().take(PREVIEW_RENDER_LINE_LIMIT) {
        let is_dir = item.ends_with('/') || item.ends_with('\\');
        let display_name = item.trim_end_matches(['/', '\\']);
        let appearance = appearance::resolve_path(
            Path::new(display_name),
            if is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
        );
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", appearance.icon),
                Style::default()
                    .fg(appearance.color)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::styled(item.clone(), Style::default().fg(palette.text)),
        ]));
    }

    PreviewContent::new(PreviewKind::Directory, lines)
        .with_detail(format!("{total_items} items"))
        .with_directory_counts(total_items, folder_count, file_count)
}

fn push_preview_line(lines: &mut Vec<Line<'static>>, label: &str, value: String) {
    let palette = appearance::palette();
    lines.push(Line::from(vec![
        Span::styled(format!("{label:<8}"), Style::default().fg(palette.muted)),
        Span::styled(value, Style::default().fg(palette.text)),
    ]));
}

fn render_plain_text_preview(text: &str) -> Vec<Line<'static>> {
    let palette = appearance::palette();
    let mut rendered = Vec::new();

    for line in collect_preview_lines(text) {
        rendered.push(Line::from(Span::styled(
            expand_tabs(&line),
            Style::default().fg(palette.text),
        )));
    }

    if rendered.is_empty() {
        rendered.push(Line::from("File is empty"));
    }
    rendered
}

fn collect_preview_lines(text: &str) -> Vec<String> {
    text.lines()
        .take(PREVIEW_RENDER_LINE_LIMIT)
        .map(trim_trailing_line_endings)
        .collect()
}

fn count_source_lines(text: &str) -> usize {
    text.lines().count().max(1)
}

fn finalize_text_preview(
    mut preview: PreviewContent,
    source_line_count: usize,
    bytes_truncated: bool,
    truncation_note: Option<String>,
) -> PreviewContent {
    if !bytes_truncated {
        preview = preview.with_source_lines(source_line_count);
    }
    if let Some(note) = truncation_note {
        preview = preview.with_truncation(note);
    }
    preview
}

fn apply_type_detail(
    mut preview: PreviewContent,
    type_detail: Option<&'static str>,
) -> PreviewContent {
    if let Some(detail) = type_detail
        && matches!(
            preview.detail.as_deref(),
            None | Some("Binary file") | Some("Read error")
        )
    {
        preview.detail = Some(detail.to_string());
    }
    preview
}

fn truncation_note(bytes_truncated: bool, line_truncated: bool) -> Option<String> {
    let mut parts = Vec::new();
    if bytes_truncated {
        parts.push("truncated to 64 KiB".to_string());
    }
    if line_truncated {
        parts.push(format!("showing first {PREVIEW_RENDER_LINE_LIMIT} lines"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  •  "))
    }
}

fn binary_preview() -> PreviewContent {
    status_preview(
        PreviewKind::Binary,
        "Binary file",
        [
            Line::from("No text preview available"),
            Line::from("Binary or unsupported file"),
        ],
    )
}

fn unavailable_preview(message: &str) -> PreviewContent {
    status_preview(
        PreviewKind::Unavailable,
        "Read error",
        [
            Line::from("Preview unavailable"),
            Line::from(message.to_string()),
        ],
    )
}

fn trim_trailing_line_endings(line: &str) -> String {
    line.trim_end_matches(['\n', '\r']).to_string()
}

fn read_text_preview(path: &Path) -> anyhow::Result<Option<TextPreview>> {
    let file = File::open(path)?;
    let mut buffer = Vec::with_capacity(PREVIEW_LIMIT_BYTES + 1);
    file.take(PREVIEW_LIMIT_BYTES as u64 + 1)
        .read_to_end(&mut buffer)?;
    let bytes_truncated = buffer.len() > PREVIEW_LIMIT_BYTES;
    if bytes_truncated {
        buffer.truncate(PREVIEW_LIMIT_BYTES);
    }

    if buffer.is_empty() {
        return Ok(Some(TextPreview {
            text: String::new(),
            bytes_truncated,
        }));
    }
    if buffer.contains(&0) {
        return Ok(None);
    }

    match String::from_utf8(buffer) {
        Ok(text) => Ok(Some(TextPreview {
            text,
            bytes_truncated,
        })),
        Err(error) if bytes_truncated && error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let bytes = error.into_bytes();
            let text = String::from_utf8(bytes[..valid_up_to].to_vec()).ok();
            Ok(text.map(|text| TextPreview {
                text,
                bytes_truncated: true,
            }))
        }
        Err(_) => Ok(None),
    }
}

fn line_number_span(number: usize, width: usize) -> Span<'static> {
    let preview = appearance::code_preview_palette();
    Span::styled(
        format!("{number:>width$} ", width = width),
        Style::default().fg(preview.line_number),
    )
}

fn line_number_width(lines: usize) -> usize {
    lines.max(1).to_string().len().max(2)
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd" | "mdx")
    )
}

fn is_iso_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if ext == "iso"
    )
}

fn is_torrent_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if ext == "torrent"
    )
}

fn parse_torrent_root(
    bytes: &[u8],
    index: &mut usize,
    metadata: &mut TorrentMetadata,
) -> Option<()> {
    expect_byte(bytes, index, b'd')?;
    while peek_byte(bytes, *index)? != b'e' {
        let key = parse_bencode_bytes(bytes, index)?;
        match key {
            b"announce" => metadata.announce = parse_bencode_string(bytes, index),
            b"comment" => metadata.comment = parse_bencode_string(bytes, index),
            b"created by" => metadata.created_by = parse_bencode_string(bytes, index),
            b"info" => parse_torrent_info(bytes, index, metadata)?,
            _ => skip_bencode_value(bytes, index)?,
        }
    }
    expect_byte(bytes, index, b'e')
}

fn parse_torrent_info(
    bytes: &[u8],
    index: &mut usize,
    metadata: &mut TorrentMetadata,
) -> Option<()> {
    expect_byte(bytes, index, b'd')?;
    while peek_byte(bytes, *index)? != b'e' {
        let key = parse_bencode_bytes(bytes, index)?;
        match key {
            b"name" => metadata.name = parse_bencode_string(bytes, index),
            b"length" => {
                let length = parse_bencode_int(bytes, index)?;
                if length >= 0 {
                    metadata.total_size = Some(length as u64);
                    if metadata.file_count == 0 {
                        metadata.file_count = 1;
                    }
                }
            }
            b"files" => parse_torrent_files(bytes, index, metadata)?,
            _ => skip_bencode_value(bytes, index)?,
        }
    }
    expect_byte(bytes, index, b'e')
}

fn parse_torrent_files(
    bytes: &[u8],
    index: &mut usize,
    metadata: &mut TorrentMetadata,
) -> Option<()> {
    expect_byte(bytes, index, b'l')?;
    let mut file_count = 0usize;
    let mut total_size = 0u64;
    while peek_byte(bytes, *index)? != b'e' {
        let length = parse_torrent_file_entry(bytes, index)?;
        file_count += 1;
        total_size = total_size.saturating_add(length);
    }
    expect_byte(bytes, index, b'e')?;
    metadata.file_count = file_count;
    metadata.total_size = Some(total_size);
    Some(())
}

fn parse_torrent_file_entry(bytes: &[u8], index: &mut usize) -> Option<u64> {
    expect_byte(bytes, index, b'd')?;
    let mut length = 0u64;
    while peek_byte(bytes, *index)? != b'e' {
        let key = parse_bencode_bytes(bytes, index)?;
        if key == b"length" {
            let value = parse_bencode_int(bytes, index)?;
            if value >= 0 {
                length = value as u64;
            }
        } else {
            skip_bencode_value(bytes, index)?;
        }
    }
    expect_byte(bytes, index, b'e')?;
    Some(length)
}

fn skip_bencode_value(bytes: &[u8], index: &mut usize) -> Option<()> {
    match peek_byte(bytes, *index)? {
        b'd' => {
            *index += 1;
            while peek_byte(bytes, *index)? != b'e' {
                parse_bencode_bytes(bytes, index)?;
                skip_bencode_value(bytes, index)?;
            }
            *index += 1;
            Some(())
        }
        b'l' => {
            *index += 1;
            while peek_byte(bytes, *index)? != b'e' {
                skip_bencode_value(bytes, index)?;
            }
            *index += 1;
            Some(())
        }
        b'i' => {
            parse_bencode_int(bytes, index)?;
            Some(())
        }
        b'0'..=b'9' => {
            parse_bencode_bytes(bytes, index)?;
            Some(())
        }
        _ => None,
    }
}

fn parse_bencode_string(bytes: &[u8], index: &mut usize) -> Option<String> {
    let value = parse_bencode_bytes(bytes, index)?;
    Some(String::from_utf8_lossy(value).into_owned())
}

fn parse_bencode_bytes<'a>(bytes: &'a [u8], index: &mut usize) -> Option<&'a [u8]> {
    let start = *index;
    while matches!(peek_byte(bytes, *index)?, b'0'..=b'9') {
        *index += 1;
    }
    let colon = *index;
    expect_byte(bytes, index, b':')?;
    let len = std::str::from_utf8(&bytes[start..colon])
        .ok()?
        .parse::<usize>()
        .ok()?;
    let end = (*index).checked_add(len)?;
    let slice = bytes.get(*index..end)?;
    *index = end;
    Some(slice)
}

fn parse_bencode_int(bytes: &[u8], index: &mut usize) -> Option<i64> {
    expect_byte(bytes, index, b'i')?;
    let start = *index;
    while peek_byte(bytes, *index)? != b'e' {
        *index += 1;
    }
    let value = std::str::from_utf8(bytes.get(start..*index)?)
        .ok()?
        .parse()
        .ok()?;
    expect_byte(bytes, index, b'e')?;
    Some(value)
}

fn peek_byte(bytes: &[u8], index: usize) -> Option<u8> {
    bytes.get(index).copied()
}

fn expect_byte(bytes: &[u8], index: &mut usize, expected: u8) -> Option<()> {
    if peek_byte(bytes, *index)? == expected {
        *index += 1;
        Some(())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("elio-preview-{label}-{unique}"))
    }

    fn file_entry(path: PathBuf) -> Entry {
        Entry {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            name_key: path.file_name().unwrap().to_string_lossy().to_lowercase(),
            path,
            kind: EntryKind::File,
            size: 0,
            modified: None,
            readonly: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn markdown_preview_formats_headings_and_lists() {
        let root = temp_path("markdown");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(&path, "# Heading\n- item\n`inline`\n").expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Markdown);
        assert_eq!(preview.lines[0].spans[0].content, "Heading");
        assert!(
            preview
                .lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content == "inline"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn markdown_preview_formats_inline_emphasis_mid_line() {
        let root = temp_path("markdown-inline");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(&path, "hello **bold** world\n").expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));
        let line = &preview.lines[0];

        assert_eq!(preview.kind, PreviewKind::Markdown);
        assert!(line.spans.iter().any(|span| span.content == "hello "));
        assert!(line.spans.iter().any(|span| span.content == "bold"));
        assert!(line.spans.iter().any(|span| span.content == " world"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn markdown_preview_renders_fenced_code_blocks() {
        let root = temp_path("markdown-fence");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(&path, "```rust\nfn main() {}\n```\n").expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Markdown);
        assert_eq!(preview.lines[0].spans[1].content, "rust");
        assert!(
            preview
                .lines
                .iter()
                .any(|line| line_text(line).contains("fn main() {}"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn markdown_preview_renders_links() {
        let root = temp_path("markdown-links");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(&path, "open [elio](https://example.com)\n").expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));
        let line = &preview.lines[0];

        assert_eq!(preview.kind, PreviewKind::Markdown);
        let link_span = line
            .spans
            .iter()
            .find(|span| span.content == "elio")
            .expect("link label should be rendered");
        assert!(link_span.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(line_text(line).contains("(https://example.com)"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn markdown_preview_adds_spacing_between_blocks() {
        let root = temp_path("markdown-spacing");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(
            &path,
            "# Heading\nParagraph text\n\n```rust\nlet x = 1;\n```\n",
        )
        .expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Markdown);
        assert!(preview.lines.iter().any(|line| line.spans.is_empty()));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn markdown_preview_renders_nested_emphasis() {
        let root = temp_path("markdown-nested");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(&path, "**bold and *italic***\n").expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));
        let line = &preview.lines[0];

        assert_eq!(preview.kind, PreviewKind::Markdown);
        let italic_span = line
            .spans
            .iter()
            .find(|span| span.content == "italic")
            .expect("nested italic content should be rendered");
        assert!(italic_span.style.add_modifier.contains(Modifier::BOLD));
        assert!(italic_span.style.add_modifier.contains(Modifier::ITALIC));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn markdown_preview_renders_mixed_lists() {
        let root = temp_path("markdown-mixed-lists");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("README.md");
        fs::write(&path, "1. first\n   - nested\n2. second\n").expect("failed to write markdown");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Markdown);
        assert!(
            preview
                .lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content == "1. "))
        );
        assert!(
            preview
                .lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content.contains("• ")))
        );
        assert!(
            preview
                .lines
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content == "2. "))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn code_preview_includes_line_numbers() {
        let root = temp_path("code");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("main.rs");
        fs::write(&path, "fn main() {}\n").expect("failed to write code");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(preview.lines[0].spans[0].content.contains("1"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn toml_preview_uses_code_renderer() {
        let root = temp_path("toml");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("config.toml");
        fs::write(&path, "name = \"elio\"\n").expect("failed to write toml");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(
            preview
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("TOML"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn desktop_preview_uses_code_renderer() {
        let root = temp_path("desktop-entry");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("app.desktop");
        fs::write(
            &path,
            "[Desktop Entry]\nName=エリオ\nName[ja]=エリオ\nExec=elio\nTerminal=false\n",
        )
        .expect("failed to write desktop entry");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(
            preview
                .detail
                .as_deref()
                .is_some_and(|detail| detail == "Desktop Entry (best-effort)")
        );
        assert!(
            preview
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains("エリオ"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn pkgbuild_preview_uses_shell_renderer() {
        let root = temp_path("pkgbuild");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("PKGBUILD");
        fs::write(
            &path,
            "pkgname=elio\nbuild() {\n  cargo build --release\n}\n",
        )
        .expect("failed to write pkgbuild");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(preview.detail.is_some());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn keys_preview_uses_fallback_renderer() {
        let root = temp_path("keys");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("bindings.keys");
        fs::write(&path, "ctrl+h=left\nctrl+l=right\n").expect("failed to write keys");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(
            preview
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("INI"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn log_preview_uses_colored_fallback_renderer() {
        let root = temp_path("log");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("server.log");
        fs::write(
            &path,
            "2026-03-10T12:00:00Z ERROR request_id=42 path=/login failed\n",
        )
        .expect("failed to write log");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(
            preview
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("Log"))
        );
        assert!(
            preview.lines[0]
                .spans
                .iter()
                .any(|span| span.content.contains("ERROR"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn torrent_preview_shows_basic_metadata() {
        let root = temp_path("torrent");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("sample.torrent");
        let bytes = b"d8:announce20:https://tracker.test7:comment12:test torrent10:created by4:elio4:infod6:lengthi12345e4:name8:file.txt12:piece lengthi262144e6:pieces20:12345678901234567890ee";
        fs::write(&path, bytes).expect("failed to write torrent");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Text);
        assert_eq!(preview.detail.as_deref(), Some("BitTorrent file"));
        assert!(
            preview
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains("file.txt"))
        );
        assert!(
            preview
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains("tracker.test"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn srt_preview_keeps_specific_type_detail() {
        let root = temp_path("srt");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("movie.srt");
        fs::write(&path, "1\n00:00:01,000 --> 00:00:02,000\nHello\n").expect("failed to write srt");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Text);
        assert_eq!(preview.detail.as_deref(), Some("SubRip subtitles"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn iso_binary_preview_keeps_specific_type_detail() {
        let root = temp_path("iso");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("disk.iso");
        fs::write(&path, [0xff, 0xfe, 0x00, 0x81]).expect("failed to write iso");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Binary);
        assert_eq!(preview.detail.as_deref(), Some("ISO disk image"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn iso_preview_lists_contents_when_bsdtar_can_read_image() {
        let root = temp_path("iso-listing");
        let image_root = root.join("image-root");
        fs::create_dir_all(image_root.join("docs")).expect("failed to create image tree");
        fs::write(image_root.join("docs/readme.txt"), "hello").expect("failed to write image file");
        let path = root.join("sample.iso");

        let created = Command::new("bsdtar")
            .arg("-cf")
            .arg(&path)
            .arg("-C")
            .arg(&image_root)
            .arg(".")
            .status();
        if !created.as_ref().is_ok_and(|status| status.success()) {
            fs::remove_dir_all(root).expect("failed to remove temp root");
            return;
        }

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Directory);
        assert_eq!(preview.item_count, Some(2));
        assert!(
            preview
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains("docs/"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn typescript_preview_uses_code_renderer() {
        let root = temp_path("typescript");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("main.ts");
        fs::write(&path, "export const value = 1;\n").expect("failed to write ts");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(preview.detail.is_some());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn tsx_preview_uses_code_renderer() {
        let root = temp_path("tsx");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("App.tsx");
        fs::write(
            &path,
            "export function App() { return <div>Hello</div>; }\n",
        )
        .expect("failed to write tsx");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(preview.detail.is_some());

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn cargo_lock_preview_uses_code_renderer() {
        let root = temp_path("cargo-lock");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("Cargo.lock");
        fs::write(&path, "version = 3\n").expect("failed to write cargo lock");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Code);
        assert!(
            preview
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("TOML"))
        );

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn text_preview_stays_plain() {
        let root = temp_path("text");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("notes.txt");
        fs::write(&path, "hello\nworld\n").expect("failed to write text");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Text);
        assert_eq!(preview.lines[0].spans.len(), 1);
        assert_eq!(preview.lines[0].spans[0].content, "hello");

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn text_preview_keeps_enough_lines_for_scrolling() {
        let root = temp_path("scroll-depth");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("long.txt");
        let text = (1..=80)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, text).expect("failed to write long text");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Text);
        assert!(preview.lines.len() >= 80);

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn utf8_preview_trims_to_last_valid_boundary() {
        let root = temp_path("utf8-boundary");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("unicode.txt");
        let bytes = [
            "a".repeat(PREVIEW_LIMIT_BYTES - 1).into_bytes(),
            "é".as_bytes().to_vec(),
        ]
        .concat();
        fs::write(&path, bytes).expect("failed to write unicode text");

        let preview = read_text_preview(&path)
            .expect("preview read should succeed")
            .expect("utf8 text should stay text");

        assert!(preview.bytes_truncated);
        assert_eq!(preview.text.len(), PREVIEW_LIMIT_BYTES - 1);
        assert!(preview.text.chars().all(|ch| ch == 'a'));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn utf8_text_file_is_not_mislabeled_as_binary() {
        let root = temp_path("utf8-text-kind");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("unicode.txt");
        let bytes = [
            "a".repeat(PREVIEW_LIMIT_BYTES - 1).into_bytes(),
            "é".as_bytes().to_vec(),
        ]
        .concat();
        fs::write(&path, bytes).expect("failed to write unicode text");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Text);
        assert!(preview.truncated);
        assert!(preview.lines.iter().all(|line| {
            line.spans
                .iter()
                .all(|span| span.content != "No text preview available")
        }));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn byte_truncated_preview_reports_truncation_without_fake_line_totals() {
        let root = temp_path("byte-truncated");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("notes.txt");
        fs::write(&path, "a".repeat(PREVIEW_LIMIT_BYTES + 32)).expect("failed to write text");

        let preview = build_preview(&file_entry(path));
        let header = preview
            .header_detail(0, 20)
            .expect("header detail should be present");

        assert_eq!(preview.kind, PreviewKind::Text);
        assert!(preview.truncated);
        assert!(preview.source_lines.is_none());
        assert!(header.contains("truncated to 64 KiB"));
        assert!(!header.contains("lines"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }

    #[test]
    fn line_truncated_preview_reports_visible_limit() {
        let root = temp_path("line-truncated");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("long.txt");
        let text = (1..=300)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, text).expect("failed to write long text");

        let preview = build_preview(&file_entry(path));
        let header = preview
            .header_detail(0, 20)
            .expect("header detail should be present");

        assert!(preview.truncated);
        assert_eq!(preview.source_lines, Some(300));
        assert!(header.contains("300 lines"));
        assert!(header.contains("showing first 240 lines"));

        fs::remove_dir_all(root).expect("failed to remove temp root");
    }
}
