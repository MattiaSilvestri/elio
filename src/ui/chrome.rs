use super::helpers;
use super::theme::Palette;
use crate::app::{App, ClipOp, FrameState};
use crate::core::Entry;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::path::Path;

pub(super) fn render_toolbar(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    state: &mut FrameState,
    palette: Palette,
) {
    helpers::fill_area(frame, area, palette.chrome, palette.text);
    let block = Block::default()
        .style(Style::default().bg(palette.chrome).fg(palette.text))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border));
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let control_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(23),
            Constraint::Min(2),
            Constraint::Length(39),
        ])
        .split(inner);
    let nav_buttons = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
        ])
        .split(control_row[0]);
    let meta = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(16),
            Constraint::Length(13),
            Constraint::Length(10),
        ])
        .split(control_row[2]);

    state.back_button = Some(nav_buttons[0]);
    state.forward_button = Some(nav_buttons[1]);
    state.parent_button = Some(nav_buttons[2]);
    state.hidden_button = Some(meta[1]);
    state.view_button = Some(meta[2]);

    helpers::render_button(
        frame,
        nav_buttons[0],
        "Back",
        "󰁍",
        app.can_go_back(),
        palette,
    );
    helpers::render_button(
        frame,
        nav_buttons[1],
        "Next",
        "󰁔",
        app.can_go_forward(),
        palette,
    );
    helpers::render_button(frame, nav_buttons[2], "Up", "󰁝", true, palette);
    frame.render_widget(
        Paragraph::new(Line::from(vec![helpers::chip_span(
            &format!("Sort: {}", app.navigation.sort_mode.label()),
            palette.button_bg,
            palette.text,
            true,
        )]))
        .alignment(Alignment::Right)
        .style(Style::default().bg(palette.chrome).fg(palette.text)),
        meta[0],
    );
    helpers::render_button(
        frame,
        meta[1],
        if app.navigation.show_hidden {
            "Hidden On"
        } else {
            "Hidden Off"
        },
        "󰈉",
        true,
        palette,
    );
    helpers::render_button(
        frame,
        meta[2],
        app.navigation.view_mode.label(),
        "󰕮",
        true,
        palette,
    );
}

// ── Status bar layout constants ───────────────────────────────────────────────

/// Minimum width for the left (chips + path) section.
const STATUS_MIN_LEFT_WIDTH: u16 = 24;
/// Right-section width when idle without entry info.
const STATUS_IDLE_RIGHT_WIDTH: u16 = 48;
/// Additional right width needed when entry info (perms/size) is shown.
const STATUS_ENTRY_INFO_WIDTH: u16 = 28;
/// Padding between clamped status message and section edge.
const STATUS_RIGHT_PADDING: usize = 2;
/// Terminal width below which entry info is hidden.
const STATUS_NARROW_THRESHOLD: u16 = 90;

// ── Pill rendering helpers ────────────────────────────────────────────────────

/// Render a "pill" span-triple: `▌ content ▐` with matching colors so the
/// half-blocks form rounded-looking ends against the bar background.
///
/// The pill occupies: 1 (▌) + 1 (space) + label + 1 (space) + 1 (▐) = label.len() + 4 columns.
/// Takes an owned `String` so callers with locally-built labels don't need to worry about lifetimes.
fn pill(label: String, pill_bg: Color, bar_bg: Color, fg: Color, bold: bool) -> Vec<Span<'static>> {
    let cap_style = Style::default().fg(pill_bg).bg(bar_bg);
    let body_style = if bold {
        Style::default().bg(pill_bg).fg(fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(pill_bg).fg(fg)
    };
    vec![
        Span::styled("▌", cap_style),
        Span::styled(format!(" {label} "), body_style),
        Span::styled("▐", cap_style),
    ]
}

/// Width consumed by `pill(label, …)` in terminal columns.
fn pill_width(label: &str) -> u16 {
    // 1 (▌) + 1 (space) + label + 1 (space) + 1 (▐)
    label.len() as u16 + 4
}

/// A subtle dot separator, dimmed, against bar_bg.
fn dot_sep(bar_bg: Color, sep_fg: Color) -> Span<'static> {
    Span::styled("  ·  ", Style::default().fg(sep_fg).bg(bar_bg))
}

/// A thin space gap between items (no visible character).
fn gap() -> Span<'static> {
    Span::raw("  ")
}

// ── Filesystem helpers ────────────────────────────────────────────────────────

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = KB * 1_024;
    const GB: u64 = MB * 1_024;
    const TB: u64 = GB * 1_024;
    if bytes >= TB {
        format!("{:.1}T", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

#[cfg(unix)]
fn mode_to_str(mode: u32) -> String {
    let type_char = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o060_000 => 'b',
        0o020_000 => 'c',
        0o010_000 => 'p',
        0o140_000 => 's',
        _ => '-',
    };
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    let mut p: Vec<char> = bits
        .iter()
        .map(|(bit, ch)| if mode & bit != 0 { *ch } else { '-' })
        .collect();
    if mode & 0o4000 != 0 {
        p[2] = if p[2] == 'x' { 's' } else { 'S' };
    }
    if mode & 0o2000 != 0 {
        p[5] = if p[5] == 'x' { 's' } else { 'S' };
    }
    if mode & 0o1000 != 0 {
        p[8] = if p[8] == 'x' { 't' } else { 'T' };
    }
    format!("{}{}", type_char, p.into_iter().collect::<String>())
}

#[cfg(unix)]
fn entry_perms_str(entry: &Entry) -> String {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::symlink_metadata(&entry.path) {
        Ok(meta) => mode_to_str(meta.permissions().mode()),
        Err(_) => "??????????".to_string(),
    }
}

#[cfg(unix)]
fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let avail = stat.f_bavail * stat.f_frsize;
    let total = stat.f_blocks * stat.f_frsize;
    Some((avail, total))
}

#[cfg(not(unix))]
fn disk_space(_path: &Path) -> Option<(u64, u64)> {
    None
}

// ── Status bar rendering ──────────────────────────────────────────────────────

pub(super) fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, palette: Palette) {
    // The bar sits on a slightly elevated background to lift it off the body.
    let bar_bg = palette.bg;
    helpers::fill_area(frame, area, bar_bg, palette.text);

    // Outer layout: left edge (1) | content | right edge (1)
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(STATUS_MIN_LEFT_WIDTH),
            Constraint::Length(1),
        ])
        .split(area);

    let edge_style = Style::default().fg(palette.accent).bg(palette.accent);
    frame.render_widget(
        Paragraph::new(Span::styled("█", edge_style)),
        outer[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled("█", edge_style)),
        outer[2],
    );

    let content_area = outer[1];
    let status_message = app.status_message();
    let wide = content_area.width >= STATUS_NARROW_THRESHOLD;
    let right_width = status_section_width(content_area.width, status_message, wide);

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(STATUS_MIN_LEFT_WIDTH),
            Constraint::Length(right_width),
        ])
        .split(content_area);

    // ── Left section ──────────────────────────────────────────────────────────
    let left_line = build_left_line(app, sections[0].width, bar_bg, palette);
    frame.render_widget(
        Paragraph::new(left_line).style(Style::default().bg(bar_bg)),
        sections[0],
    );

    // ── Right section ─────────────────────────────────────────────────────────
    let right_line = build_right_line(app, wide, status_message, sections[1].width, bar_bg, palette);
    frame.render_widget(
        Paragraph::new(right_line)
            .alignment(Alignment::Right)
            .style(Style::default().bg(bar_bg)),
        sections[1],
    );
}

fn build_left_line(app: &App, section_width: u16, bar_bg: Color, palette: Palette) -> Line<'static> {
    let clip = app.clipboard_info();
    let sel_count = app.selection_count();
    let paste_prog = app.paste_progress();
    let queued_pastes = app.queued_paste_count();
    let trash_prog = app.trash_progress();
    let restore_prog = app.restore_progress();

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used_width: u16 = 0;

    // Leading space so pills don't butt up against the terminal edge.
    spans.push(Span::raw(" "));
    used_width += 1;

    // ── Progress / clipboard chip (mutually exclusive priority) ───────────────
    let action_chip: Option<(String, Color)> =
        if let Some((completed, total, permanent)) = trash_prog {
            let label = if permanent {
                format!("󰩺  Deleting {completed}/{total}")
            } else {
                let noun = if total == 1 { "item" } else { "items" };
                format!("󰩺  Trashing {total} {noun}…")
            };
            Some((label, palette.trash_bar))
        } else if let Some((completed, total)) = restore_prog {
            let noun = if total == 1 { "item" } else { "items" };
            Some((
                format!("  Restoring {completed}/{total} {noun}"),
                palette.restore_bar,
            ))
        } else if let Some((completed, total, op)) = paste_prog {
            let (verb, icon, color) = match op {
                ClipOp::Yank => ("Copying", "󰆏", palette.yank_bar),
                ClipOp::Cut => ("Moving", "󰆐", palette.cut_bar),
            };
            let label = if queued_pastes == 0 {
                format!("{icon}  {verb} {completed}/{total}")
            } else {
                format!("{icon}  {verb} {completed}/{total}  +{queued_pastes} queued")
            };
            Some((label, color))
        } else if let Some((clip_count, clip_op)) = clip {
            match clip_op {
                ClipOp::Yank => Some((
                    format!("󰆏  {clip_count} yanked"),
                    palette.yank_bar,
                )),
                ClipOp::Cut => Some((
                    format!("󰆐  {clip_count} cut"),
                    palette.cut_bar,
                )),
            }
        } else {
            None
        };

    if let Some((label, color)) = action_chip {
        let w = pill_width(&label);
        used_width += w + 2; // pill + gap
        spans.extend(pill(label, color, bar_bg, palette.chrome, true));
        spans.push(gap());
    }

    // ── Selection chip ────────────────────────────────────────────────────────
    if sel_count > 0 {
        let label = format!("  {sel_count} selected");
        let w = pill_width(&label);
        used_width += w + 2;
        spans.extend(pill(
            label,
            palette.selection_bar,
            bar_bg,
            palette.chrome,
            true,
        ));
        spans.push(gap());
    }

    // ── Path / position summary ───────────────────────────────────────────────
    // Split into index/total "position" badge and the filename separately.
    let summary = app.selection_summary();
    // summary format: "N/M  filename"  or "0/0  /path"
    let (pos_part, name_part) = if let Some(idx) = summary.find("  ") {
        (&summary[..idx], summary[idx + 2..].trim())
    } else {
        (summary.as_str(), "")
    };

    let name_width = section_width.saturating_sub(used_width) as usize;
    if name_width > 4 {
        // Position badge: accent-colored, slightly dimmed
        if !pos_part.is_empty() {
            spans.push(Span::styled(
                pos_part.to_string(),
                Style::default()
                    .fg(palette.accent)
                    .bg(bar_bg)
                    .add_modifier(Modifier::DIM),
            ));
            spans.push(Span::styled(
                "  ",
                Style::default().bg(bar_bg),
            ));
        }
        // Filename: bold, full text color
        if !name_part.is_empty() {
            let truncated = helpers::truncate_middle(name_part, name_width.saturating_sub(6));
            spans.push(Span::styled(
                truncated,
                Style::default()
                    .fg(palette.text)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        // Very narrow: fallback to full summary
        let truncated = helpers::truncate_middle(&summary, name_width.max(1));
        spans.push(Span::styled(
            truncated,
            Style::default()
                .fg(palette.text)
                .bg(bar_bg)
                .add_modifier(Modifier::BOLD),
        ));
    }

    Line::from(spans)
}

#[allow(unused_variables)]
fn build_right_line(
    app: &App,
    wide: bool,
    status_message: &str,
    right_width: u16,
    bar_bg: Color,
    palette: Palette,
) -> Line<'static> {
    let muted = Style::default().fg(palette.muted).bg(bar_bg);

    let mut spans: Vec<Span<'static>> = Vec::new();

    // ── Entry info: permissions + size ────────────────────────────────────────
    #[cfg(unix)]
    if wide
        && let Some(entry) = app.selected_entry()
    {
        let perms = entry_perms_str(entry);
        // Colour each permission character individually for readability:
        // type char → accent dim, rwx blocks → muted/text
        let perms_spans = colour_perms(&perms, bar_bg, palette);
        spans.extend(perms_spans);
        spans.push(Span::raw("  "));

        let size_str = if entry.is_dir() {
            "  dir".to_string()
        } else {
            format_bytes(entry.size)
        };
        spans.push(Span::styled(size_str, muted));
        spans.push(dot_sep(bar_bg, palette.border));
    }

    // ── Disk space ────────────────────────────────────────────────────────────
    if let Some((avail, total)) = disk_space(&app.navigation.cwd) {
        // Show as "12.3G free · 256G"
        spans.push(Span::styled(format_bytes(avail), Style::default().fg(palette.text).bg(bar_bg)));
        spans.push(Span::styled(" free · ", Style::default().fg(palette.muted).bg(bar_bg)));
        spans.push(Span::styled(format_bytes(total), muted));
        spans.push(dot_sep(bar_bg, palette.border));
    }

    // ── Idle hint or status message ───────────────────────────────────────────
    let hint = if status_message.is_empty() {
        status_idle_hint().to_string()
    } else {
        helpers::clamp_label(
            status_message,
            (right_width as usize).saturating_sub(STATUS_RIGHT_PADDING),
        )
    };
    spans.push(Span::styled(hint, muted));
    // Trailing space so text doesn't crowd the right edge.
    spans.push(Span::styled(" ", Style::default().bg(bar_bg)));

    Line::from(spans)
}

/// Colour-code a 10-char Unix permissions string for visual clarity:
///   - type char  → accent (dimmed)
///   - 'r'        → warm amber  (palette.selection_bar)
///   - 'w'        → soft red    (palette.cut_bar, dimmed)
///   - 'x'/'s'/'t'→ soft green  (palette.yank_bar)
///   - '-'        → muted/dim
fn colour_perms(perms: &str, bar_bg: Color, palette: Palette) -> Vec<Span<'static>> {
    perms
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            let style = if i == 0 {
                // type character
                Style::default()
                    .fg(palette.accent)
                    .bg(bar_bg)
                    .add_modifier(Modifier::DIM)
            } else {
                match ch {
                    'r' => Style::default().fg(palette.selection_bar).bg(bar_bg),
                    'w' => Style::default()
                        .fg(palette.cut_bar)
                        .bg(bar_bg)
                        .add_modifier(Modifier::DIM),
                    'x' | 's' | 't' => Style::default().fg(palette.yank_bar).bg(bar_bg),
                    'S' | 'T' => Style::default()
                        .fg(palette.yank_bar)
                        .bg(bar_bg)
                        .add_modifier(Modifier::DIM),
                    _ => Style::default()
                        .fg(palette.border)
                        .bg(bar_bg)
                        .add_modifier(Modifier::DIM),
                }
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn status_section_width(total_width: u16, status_message: &str, wide: bool) -> u16 {
    let base = if wide {
        STATUS_IDLE_RIGHT_WIDTH + STATUS_ENTRY_INFO_WIDTH
    } else {
        STATUS_IDLE_RIGHT_WIDTH
    };
    let max_right = total_width.saturating_sub(STATUS_MIN_LEFT_WIDTH).max(1);

    if status_message.is_empty() {
        return base.min(max_right).max(1);
    }

    let desired =
        helpers::display_width(status_message).saturating_add(STATUS_RIGHT_PADDING) + base as usize;
    desired.max(base as usize).min(max_right as usize).max(1) as u16
}

fn status_idle_hint() -> &'static str {
    "f folders  ^F files  ? help"
}

#[cfg(test)]
mod tests {
    use super::{render_status, status_idle_hint, status_section_width};
    use crate::{
        app::{App, FrameState},
        ui::{helpers, theme},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent};
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
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
        std::env::temp_dir().join(format!("elio-chrome-{label}-{unique}"))
    }

    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    }

    #[test]
    fn idle_status_keeps_the_compact_help_width() {
        assert!(status_section_width(100, "", false) >= 34);
    }

    #[test]
    fn real_status_messages_expand_beyond_the_idle_width() {
        assert!(status_section_width(100, "Clipboard helper not found while copying", false) > 34);
    }

    #[test]
    fn narrow_status_messages_truncate_at_the_end() {
        let rendered = helpers::clamp_label("Clipboard helper not found", 18);
        assert_eq!(rendered, "Clipboard helper …");
    }

    #[test]
    fn idle_hint_stays_unchanged() {
        assert_eq!(status_idle_hint(), "f folders  ^F files  ? help");
    }

    #[test]
    fn paste_status_chip_shows_queued_count() {
        let src_dir = temp_path("paste-chip-src");
        let dst_dir = temp_path("paste-chip-dst");
        fs::create_dir_all(&src_dir).expect("failed to create source dir");
        fs::create_dir_all(&dst_dir).expect("failed to create destination dir");
        fs::write(src_dir.join("a.txt"), "a").expect("failed to write first file");
        fs::write(src_dir.join("b.txt"), "b").expect("failed to write second file");

        let mut app = App::new_at(src_dir.clone()).expect("failed to create app");
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('y'))))
            .expect("yank shortcut should succeed");
        app.navigation.cwd = dst_dir.clone();
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('p'))))
            .expect("paste shortcut should succeed");
        app.navigation.cwd = src_dir.clone();
        app.navigation.selected = 1;
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('y'))))
            .expect("second yank shortcut should succeed");
        app.navigation.cwd = dst_dir.clone();
        app.handle_event(Event::Key(KeyEvent::from(KeyCode::Char('p'))))
            .expect("second paste should be queued");

        let mut terminal = Terminal::new(TestBackend::new(200, 1)).expect("terminal should init");
        terminal
            .draw(|frame| render_status(frame, frame.area(), &app, theme::palette()))
            .expect("status should render");

        let rendered = row_text(terminal.backend().buffer(), 0);
        assert!(
            rendered.contains("queued"),
            "status row should show queued paste count, got: {rendered:?}"
        );

        app.set_frame_state(FrameState::default());
        drop(app);
        fs::remove_dir_all(src_dir).expect("failed to remove source dir");
        fs::remove_dir_all(dst_dir).expect("failed to remove destination dir");
    }
}
