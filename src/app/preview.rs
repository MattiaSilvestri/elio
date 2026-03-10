use super::*;
use crate::appearance;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    str::FromStr,
    sync::OnceLock,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color as SyntectColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem,
        ThemeSettings,
    },
    parsing::{SyntaxReference, SyntaxSet},
};

const PREVIEW_LIMIT_BYTES: usize = 64 * 1024;
const PREVIEW_RENDER_LINE_LIMIT: usize = 240;
static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static CODE_THEME: OnceLock<Theme> = OnceLock::new();

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
            Self::Binary => "Preview",
            Self::Unavailable => "Preview",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PreviewContent {
    pub kind: PreviewKind,
    pub detail: Option<String>,
    pub source_lines: Option<usize>,
    pub item_count: Option<usize>,
    pub folder_count: Option<usize>,
    pub file_count: Option<usize>,
    pub lines: Vec<Line<'static>>,
}

impl PreviewContent {
    pub(super) fn new(kind: PreviewKind, lines: Vec<Line<'static>>) -> Self {
        Self {
            kind,
            detail: None,
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

    pub(super) fn lines_for_viewport(&self, offset: usize, max_lines: usize) -> Vec<Line<'static>> {
        self.lines
            .iter()
            .skip(offset)
            .take(max_lines)
            .cloned()
            .collect()
    }

    pub(super) fn header_detail(&self, offset: usize, visible_rows: usize) -> Option<String> {
        if self.kind == PreviewKind::Directory {
            return self.detail.clone();
        }

        if let Some(source_lines) = self.source_lines {
            let summary = format!("{source_lines} lines");
            return match &self.detail {
                Some(detail) if !detail.is_empty() => Some(format!("{detail}  •  {summary}")),
                _ => Some(summary),
            };
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

    let text = match read_text_preview(&entry.path) {
        Ok(Some(text)) => text,
        Ok(None) => return binary_preview(),
        Err(_) => return unavailable_preview("The file could not be read"),
    };
    let source_line_count = count_source_lines(&text);

    if is_markdown_path(&entry.path) {
        return PreviewContent::new(PreviewKind::Markdown, render_markdown_preview(&text))
            .with_source_lines(source_line_count);
    }

    if appearance::classify_path(&entry.path, entry.kind) == FileClass::Code {
        let syntax = code_syntax_for(&entry.path, None, syntax_set());
        return PreviewContent::new(
            PreviewKind::Code,
            render_code_preview(&entry.path, &text, None, true),
        )
        .with_detail(syntax.name.clone())
        .with_source_lines(source_line_count);
    }

    PreviewContent::new(PreviewKind::Text, render_plain_text_preview(&text))
        .with_source_lines(source_line_count)
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
                            .add_modifier(Modifier::BOLD),
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

fn render_plain_text_preview(text: &str) -> Vec<Line<'static>> {
    let palette = appearance::palette();
    let lines = collect_preview_lines(text);
    let number_width = line_number_width(lines.len());
    let mut rendered = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        rendered.push(Line::from(vec![
            line_number_span(index + 1, number_width, palette),
            Span::styled(expand_tabs(line), Style::default().fg(palette.text)),
        ]));
    }

    if rendered.is_empty() {
        rendered.push(Line::from("File is empty"));
    }
    rendered
}

fn render_markdown_preview(text: &str) -> Vec<Line<'static>> {
    let palette = appearance::palette();
    let mut rendered = Vec::new();
    let mut fence_lang = None::<String>;
    let mut fence_lines = Vec::new();

    for raw_line in text.lines() {
        if rendered.len() >= PREVIEW_RENDER_LINE_LIMIT {
            break;
        }

        if let Some(lang) = fence_lang.as_ref() {
            if is_fence_delimiter(raw_line) {
                rendered.extend(render_markdown_fence(lang, &fence_lines));
                fence_lang = None;
                fence_lines.clear();
                continue;
            }
            fence_lines.push(raw_line.to_string());
            continue;
        }

        if let Some(lang) = parse_fence_start(raw_line) {
            fence_lang = Some(lang);
            continue;
        }

        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            rendered.push(Line::from(String::new()));
            continue;
        }

        if let Some((level, title)) = parse_heading(raw_line) {
            rendered.push(render_heading_line(level, title, palette));
            if level <= 2 && rendered.len() < PREVIEW_RENDER_LINE_LIMIT {
                rendered.push(render_heading_rule(title, level, palette));
            }
            continue;
        }

        if is_thematic_break(trimmed) {
            rendered.push(Line::from(Span::styled(
                "────────────────",
                Style::default().fg(palette.border),
            )));
            continue;
        }

        if let Some(quoted) = trimmed.strip_prefix('>') {
            rendered.push(render_quote_line(quoted.trim_start(), palette));
            continue;
        }

        if let Some((checked, body, indent)) = parse_task_item(raw_line) {
            rendered.push(render_list_item(
                if checked { "󰄬" } else { "󰄱" },
                body,
                indent,
                palette,
            ));
            continue;
        }

        if let Some((body, indent)) = parse_unordered_item(raw_line) {
            rendered.push(render_list_item("•", body, indent, palette));
            continue;
        }

        if let Some((number, body, indent)) = parse_ordered_item(raw_line) {
            rendered.push(render_list_item(
                &format!("{number}."),
                body,
                indent,
                palette,
            ));
            continue;
        }

        rendered.push(Line::from(parse_inline_markdown(
            raw_line.trim_end(),
            palette,
        )));
    }

    if fence_lang.is_some() && rendered.len() < PREVIEW_RENDER_LINE_LIMIT {
        rendered.extend(render_markdown_fence(
            fence_lang.as_deref().unwrap_or("text"),
            &fence_lines,
        ));
    }

    rendered.truncate(PREVIEW_RENDER_LINE_LIMIT);
    if rendered.is_empty() {
        rendered.push(Line::from("File is empty"));
    }
    rendered
}

fn render_markdown_fence(language: &str, lines: &[String]) -> Vec<Line<'static>> {
    let palette = appearance::palette();
    let mut rendered = vec![Line::from(vec![
        Span::styled(
            "󰆍 ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            language.to_string(),
            Style::default()
                .fg(palette.muted)
                .add_modifier(Modifier::ITALIC),
        ),
    ])];
    rendered.extend(render_plain_fence_body(lines));
    rendered
}

fn render_plain_fence_body(lines: &[String]) -> Vec<Line<'static>> {
    let palette = appearance::palette();
    let mut rendered = Vec::new();

    for line in lines
        .iter()
        .take(PREVIEW_RENDER_LINE_LIMIT.saturating_sub(1))
    {
        rendered.push(Line::from(vec![
            Span::styled("│ ", Style::default().fg(palette.border)),
            Span::styled(expand_tabs(line), Style::default().fg(palette.text)),
        ]));
    }

    if rendered.is_empty() {
        rendered.push(Line::from(vec![
            Span::styled("│ ", Style::default().fg(palette.border)),
            Span::styled("Code block is empty", Style::default().fg(palette.muted)),
        ]));
    }

    rendered
}

fn render_code_preview(
    path: &Path,
    text: &str,
    language: Option<&str>,
    line_numbers: bool,
) -> Vec<Line<'static>> {
    let palette = appearance::palette();
    let syntax_set = syntax_set();
    let syntax = code_syntax_for(path, language, syntax_set);
    let mut highlighter = HighlightLines::new(syntax, code_theme());

    let source_lines = collect_preview_lines(text);
    let number_width = line_number_width(source_lines.len());
    let mut rendered = Vec::new();

    for (index, line) in source_lines.iter().enumerate() {
        let mut spans = Vec::new();
        if line_numbers {
            spans.push(line_number_span(index + 1, number_width, palette));
        } else {
            spans.push(Span::styled("│ ", Style::default().fg(palette.border)));
        }

        match highlighter.highlight_line(line, syntax_set) {
            Ok(ranges) => {
                for (style, segment) in ranges {
                    spans.push(Span::styled(
                        expand_tabs(segment),
                        ratatui_style_from_syntect(style),
                    ));
                }
            }
            Err(_) => spans.push(Span::styled(
                expand_tabs(line),
                Style::default().fg(palette.text),
            )),
        }
        rendered.push(Line::from(spans));
    }

    if rendered.is_empty() {
        rendered.push(Line::from("File is empty"));
    }
    rendered
}

fn render_heading_line(level: usize, title: &str, palette: appearance::Palette) -> Line<'static> {
    let color = match level {
        1 => palette.accent_text,
        2 => palette.accent,
        3 => palette.text,
        _ => palette.muted,
    };
    Line::from(parse_inline_markdown_with_style(
        title.trim(),
        palette,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn render_heading_rule(title: &str, level: usize, palette: appearance::Palette) -> Line<'static> {
    let len = title.trim().chars().count().clamp(8, 18);
    let color = if level == 1 {
        palette.accent
    } else {
        palette.border
    };
    Line::from(Span::styled("─".repeat(len), Style::default().fg(color)))
}

fn render_quote_line(text: &str, palette: appearance::Palette) -> Line<'static> {
    let mut spans = vec![Span::styled("▎ ", Style::default().fg(palette.accent))];
    spans.extend(parse_inline_markdown(text, palette));
    Line::from(spans)
}

fn render_list_item(
    marker: &str,
    body: &str,
    indent: usize,
    palette: appearance::Palette,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" ".repeat(indent * 2)),
        Span::styled(
            format!("{marker} "),
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    spans.extend(parse_inline_markdown(body.trim(), palette));
    Line::from(spans)
}

fn parse_inline_markdown(input: &str, palette: appearance::Palette) -> Vec<Span<'static>> {
    parse_inline_markdown_with_style(input, palette, Style::default().fg(palette.text))
}

fn parse_inline_markdown_with_style(
    input: &str,
    palette: appearance::Palette,
    base_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = input;

    while !rest.is_empty() {
        if let Some((content, remainder)) = take_delimited(rest, "`") {
            spans.push(Span::styled(
                content.to_string(),
                Style::default()
                    .fg(palette.accent_text)
                    .bg(palette.accent_soft)
                    .add_modifier(Modifier::BOLD),
            ));
            rest = remainder;
            continue;
        }

        if let Some((content, remainder)) = take_delimited(rest, "**") {
            spans.push(Span::styled(
                content.to_string(),
                base_style.add_modifier(Modifier::BOLD),
            ));
            rest = remainder;
            continue;
        }

        if let Some((content, remainder)) = take_delimited(rest, "*") {
            spans.push(Span::styled(
                content.to_string(),
                base_style.add_modifier(Modifier::ITALIC),
            ));
            rest = remainder;
            continue;
        }

        if let Some((content, remainder)) = take_delimited(rest, "~~") {
            spans.push(Span::styled(
                content.to_string(),
                base_style.add_modifier(Modifier::CROSSED_OUT),
            ));
            rest = remainder;
            continue;
        }

        if let Some(((label, url), remainder)) = take_link(rest) {
            spans.push(Span::styled(
                label.to_string(),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ));
            spans.push(Span::styled(
                format!(" 󰌹 {}", clamp_preview_text(url, 28)),
                Style::default().fg(palette.muted),
            ));
            rest = remainder;
            continue;
        }

        let next = next_inline_marker(rest).unwrap_or(rest.len());
        let (segment, remainder) = rest.split_at(next);
        if !segment.is_empty() {
            spans.push(Span::styled(segment.to_string(), base_style));
        }
        if remainder.is_empty() {
            break;
        }
        spans.push(Span::styled(remainder[..1].to_string(), base_style));
        rest = &remainder[1..];
    }

    spans
}

fn next_inline_marker(input: &str) -> Option<usize> {
    ['`', '[', '*', '~']
        .into_iter()
        .filter_map(|needle| input.find(needle))
        .min()
}

fn take_delimited<'a>(input: &'a str, delimiter: &str) -> Option<(&'a str, &'a str)> {
    let stripped = input.strip_prefix(delimiter)?;
    let end = stripped.find(delimiter)?;
    Some((&stripped[..end], &stripped[end + delimiter.len()..]))
}

fn take_link(input: &str) -> Option<((&str, &str), &str)> {
    let stripped = input.strip_prefix('[')?;
    let label_end = stripped.find("](")?;
    let url_end = stripped[label_end + 2..].find(')')?;
    let label = &stripped[..label_end];
    let url = &stripped[label_end + 2..label_end + 2 + url_end];
    let remainder = &stripped[label_end + 3 + url_end..];
    Some(((label, url), remainder))
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|&ch| ch == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    trimmed[level..]
        .strip_prefix(' ')
        .map(|title| (level, title))
}

fn parse_fence_start(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let stripped = trimmed
        .strip_prefix("```")
        .or_else(|| trimmed.strip_prefix("~~~"))?;
    Some(stripped.trim().to_string())
}

fn is_fence_delimiter(line: &str) -> bool {
    matches!(line.trim(), "```" | "~~~")
}

fn is_thematic_break(line: &str) -> bool {
    let compact = line
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    compact.len() >= 3 && compact.chars().all(|ch| matches!(ch, '-' | '*' | '_'))
}

fn parse_task_item(line: &str) -> Option<(bool, &str, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len().saturating_sub(trimmed.len()) / 2;
    parse_prefixed_item(trimmed, ["- [ ] ", "* [ ] ", "+ [ ] "])
        .map(|body| (false, body, indent))
        .or_else(|| {
            parse_prefixed_item(
                trimmed,
                ["- [x] ", "* [x] ", "+ [x] ", "- [X] ", "* [X] ", "+ [X] "],
            )
            .map(|body| (true, body, indent))
        })
}

fn parse_unordered_item(line: &str) -> Option<(&str, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len().saturating_sub(trimmed.len()) / 2;
    parse_prefixed_item(trimmed, ["- ", "* ", "+ "]).map(|body| (body, indent))
}

fn parse_ordered_item(line: &str) -> Option<(&str, &str, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len().saturating_sub(trimmed.len()) / 2;
    let digits = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits == 0 || !trimmed[digits..].starts_with(". ") {
        return None;
    }
    Some((&trimmed[..digits], &trimmed[digits + 2..], indent))
}

fn parse_prefixed_item<'a, const N: usize>(input: &'a str, prefixes: [&str; N]) -> Option<&'a str> {
    prefixes
        .into_iter()
        .find_map(|prefix| input.strip_prefix(prefix))
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

fn read_text_preview(path: &Path) -> anyhow::Result<Option<String>> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0; PREVIEW_LIMIT_BYTES];
    let count = file.read(&mut buffer)?;
    buffer.truncate(count);

    if buffer.is_empty() {
        return Ok(Some(String::new()));
    }
    if buffer.contains(&0) {
        return Ok(None);
    }

    match String::from_utf8(buffer) {
        Ok(text) => Ok(Some(text)),
        Err(_) => Ok(None),
    }
}

fn code_syntax_for<'a>(
    path: &Path,
    language: Option<&str>,
    syntax_set: &'a SyntaxSet,
) -> &'a SyntaxReference {
    if let Some(language) = language
        && let Some(syntax) = syntax_set.find_syntax_by_token(language)
    {
        return syntax;
    }

    if let Ok(Some(syntax)) = syntax_set.find_syntax_for_file(path) {
        return syntax;
    }

    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| syntax_set.find_syntax_by_extension(extension))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn code_theme() -> &'static Theme {
    CODE_THEME.get_or_init(build_code_theme)
}

fn build_code_theme() -> Theme {
    Theme {
        name: Some("elio-preview".to_string()),
        author: Some("Elio".to_string()),
        settings: ThemeSettings {
            foreground: Some(syntect_color(0xd7, 0xe3, 0xf4)),
            background: Some(syntect_color(0x0a, 0x0d, 0x12)),
            selection: Some(syntect_color(0x12, 0x2a, 0x3f)),
            selection_foreground: Some(syntect_color(0xf2, 0xf7, 0xff)),
            caret: Some(syntect_color(0x12, 0xd2, 0xff)),
            line_highlight: Some(syntect_color(0x10, 0x15, 0x1f)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            theme_item(
                "comment",
                Some((0x6f, 0x83, 0x99)),
                None,
                Some(FontStyle::ITALIC),
            ),
            theme_item("string", Some((0x79, 0xe7, 0xd5)), None, None),
            theme_item(
                "constant.numeric, constant.language, constant.character.escape",
                Some((0xff, 0xa6, 0x57)),
                None,
                None,
            ),
            theme_item(
                "keyword, storage",
                Some((0xff, 0x78, 0xc6)),
                None,
                Some(FontStyle::BOLD),
            ),
            theme_item(
                "entity.name.function, support.function",
                Some((0x38, 0xd5, 0xff)),
                None,
                None,
            ),
            theme_item(
                "entity.name.type, support.type, support.class",
                Some((0xb3, 0x8c, 0xff)),
                None,
                None,
            ),
            theme_item("variable.parameter", Some((0xff, 0xd8, 0x66)), None, None),
            theme_item(
                "entity.name.tag, meta.tag",
                Some((0x59, 0xde, 0x94)),
                None,
                None,
            ),
            theme_item("keyword.operator", Some((0x72, 0xe3, 0xff)), None, None),
            theme_item(
                "invalid",
                Some((0xff, 0x85, 0x85)),
                None,
                Some(FontStyle::BOLD),
            ),
        ],
    }
}

fn theme_item(
    selectors: &str,
    foreground: Option<(u8, u8, u8)>,
    background: Option<(u8, u8, u8)>,
    font_style: Option<FontStyle>,
) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(selectors).expect("preview theme selectors should parse"),
        style: StyleModifier {
            foreground: foreground.map(|(r, g, b)| syntect_color(r, g, b)),
            background: background.map(|(r, g, b)| syntect_color(r, g, b)),
            font_style,
        },
    }
}

fn syntect_color(r: u8, g: u8, b: u8) -> SyntectColor {
    SyntectColor { r, g, b, a: 0xFF }
}

fn ratatui_style_from_syntect(style: syntect::highlighting::Style) -> Style {
    let mut ratatui = Style::default().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));

    if style.font_style.contains(FontStyle::BOLD) {
        ratatui = ratatui.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        ratatui = ratatui.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        ratatui = ratatui.add_modifier(Modifier::UNDERLINED);
    }

    ratatui
}

fn line_number_span(number: usize, width: usize, palette: appearance::Palette) -> Span<'static> {
    Span::styled(
        format!("{number:>width$} ", width = width),
        Style::default().fg(palette.muted),
    )
}

fn line_number_width(lines: usize) -> usize {
    lines.max(1).to_string().len().max(2)
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

fn clamp_preview_text(text: &str, max: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max {
        return text.to_string();
    }

    chars[..max.saturating_sub(1)].iter().collect::<String>() + "…"
}

fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd" | "mdx")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
            hidden: false,
        }
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
    fn text_preview_includes_line_numbers() {
        let root = temp_path("text");
        fs::create_dir_all(&root).expect("failed to create temp root");
        let path = root.join("notes.txt");
        fs::write(&path, "hello\nworld\n").expect("failed to write text");

        let preview = build_preview(&file_entry(path));

        assert_eq!(preview.kind, PreviewKind::Text);
        assert!(preview.lines[0].spans[0].content.contains("1"));
        assert_eq!(preview.lines[0].spans[1].content, "hello");

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
}
