use super::common::{next_non_whitespace_char, scan_string, styled_text};
use crate::ui::theme;
use ratatui::{style::Modifier, text::Span};

pub(super) enum LuaState {
    None,
    BlockComment(String),
    LongString(String),
}

impl Default for LuaState {
    fn default() -> Self {
        Self::None
    }
}

pub(super) fn highlight_lua_line(
    line: &str,
    palette: theme::CodePreviewPalette,
    state: &mut LuaState,
) -> Vec<Span<'static>> {
    if matches!(state, LuaState::None)
        && let Some(rest) = line.strip_prefix("#!")
    {
        return vec![
            styled_text("#!", palette.r#macro, Modifier::BOLD),
            styled_text(rest, palette.string, Modifier::empty()),
        ];
    }

    let mut spans = Vec::new();
    let mut index = 0usize;

    while index < line.len() {
        match state {
            LuaState::BlockComment(close_delim) => {
                if let Some(end) = find_long_bracket_close(line, index, close_delim) {
                    spans.push(styled_text(
                        &line[index..end],
                        palette.comment,
                        Modifier::ITALIC,
                    ));
                    *state = LuaState::None;
                    index = end;
                    continue;
                }
                spans.push(styled_text(
                    &line[index..],
                    palette.comment,
                    Modifier::ITALIC,
                ));
                return spans;
            }
            LuaState::LongString(close_delim) => {
                if let Some(end) = find_long_bracket_close(line, index, close_delim) {
                    spans.push(styled_text(
                        &line[index..end],
                        palette.string,
                        Modifier::empty(),
                    ));
                    *state = LuaState::None;
                    index = end;
                    continue;
                }
                spans.push(styled_text(
                    &line[index..],
                    palette.string,
                    Modifier::empty(),
                ));
                return spans;
            }
            LuaState::None => {}
        }

        let ch = line[index..].chars().next().unwrap_or(' ');
        if ch.is_whitespace() {
            let start = index;
            while let Some(current) = line[index..].chars().next() {
                if !current.is_whitespace() {
                    break;
                }
                index += current.len_utf8();
            }
            spans.push(Span::raw(line[start..index].to_string()));
            continue;
        }

        if line[index..].starts_with("--") {
            if let Some((content_start, close_delim)) = long_bracket_delimiter_at(line, index + 2) {
                if let Some(end) = find_long_bracket_close(line, content_start, &close_delim) {
                    spans.push(styled_text(
                        &line[index..end],
                        palette.comment,
                        Modifier::ITALIC,
                    ));
                    index = end;
                } else {
                    spans.push(styled_text(
                        &line[index..],
                        palette.comment,
                        Modifier::ITALIC,
                    ));
                    *state = LuaState::BlockComment(close_delim);
                    return spans;
                }
            } else {
                spans.push(styled_text(
                    &line[index..],
                    palette.comment,
                    Modifier::ITALIC,
                ));
                return spans;
            }
            continue;
        }

        if let Some((content_start, close_delim)) = long_bracket_delimiter_at(line, index) {
            if let Some(end) = find_long_bracket_close(line, content_start, &close_delim) {
                spans.push(styled_text(
                    &line[index..end],
                    palette.string,
                    Modifier::empty(),
                ));
                index = end;
            } else {
                spans.push(styled_text(
                    &line[index..],
                    palette.string,
                    Modifier::empty(),
                ));
                *state = LuaState::LongString(close_delim);
                return spans;
            }
            continue;
        }

        if matches!(ch, '"' | '\'') {
            let end = scan_string(line, index, ch);
            spans.push(styled_text(
                &line[index..end],
                palette.string,
                Modifier::empty(),
            ));
            index = end;
            continue;
        }

        if line[index..].starts_with("...") {
            spans.push(styled_text("...", palette.parameter, Modifier::empty()));
            index += 3;
            continue;
        }

        if ch.is_ascii_digit() || looks_like_fractional_literal(line, index) {
            let end = scan_lua_number(line, index);
            spans.push(styled_text(
                &line[index..end],
                palette.constant,
                Modifier::empty(),
            ));
            index = end;
            continue;
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = index;
            index += ch.len_utf8();
            while let Some(current) = line[index..].chars().next() {
                if current.is_ascii_alphanumeric() || current == '_' {
                    index += current.len_utf8();
                } else {
                    break;
                }
            }

            let token = &line[start..index];
            let next = next_non_whitespace_char(line, index);
            let (color, modifier) = if is_lua_keyword(token) {
                (palette.keyword, Modifier::BOLD)
            } else if is_lua_constant(token) {
                (palette.constant, Modifier::empty())
            } else if token == "self" {
                (palette.parameter, Modifier::empty())
            } else if is_lua_call_start(next) {
                (palette.function, Modifier::empty())
            } else {
                (palette.fg, Modifier::empty())
            };
            spans.push(styled_text(token, color, modifier));
            continue;
        }

        let end = consume_lua_operator(line, index);
        spans.push(styled_text(
            &line[index..end],
            palette.operator,
            Modifier::empty(),
        ));
        index = end;
    }

    spans
}

fn is_lua_keyword(token: &str) -> bool {
    matches!(
        token,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "for"
            | "function"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "until"
            | "while"
    )
}

fn is_lua_constant(token: &str) -> bool {
    matches!(token, "false" | "nil" | "true")
}

fn is_lua_call_start(next: Option<char>) -> bool {
    matches!(next, Some('(' | '{' | '"' | '\''))
}

fn long_bracket_delimiter_at(input: &str, start: usize) -> Option<(usize, String)> {
    let bytes = input.as_bytes();
    if bytes.get(start).copied()? != b'[' {
        return None;
    }

    let mut index = start + 1;
    while matches!(bytes.get(index).copied(), Some(b'=')) {
        index += 1;
    }
    if !matches!(bytes.get(index).copied(), Some(b'[')) {
        return None;
    }

    let equals = index.saturating_sub(start + 1);
    index += 1;
    Some((index, format!("]{}]", "=".repeat(equals))))
}

fn find_long_bracket_close(input: &str, start: usize, close_delim: &str) -> Option<usize> {
    input[start..]
        .find(close_delim)
        .map(|offset| start + offset + close_delim.len())
}

fn looks_like_fractional_literal(input: &str, start: usize) -> bool {
    let bytes = input.as_bytes();
    matches!(bytes.get(start).copied(), Some(b'.'))
        && !matches!(bytes.get(start + 1).copied(), Some(b'.'))
        && matches!(bytes.get(start + 1).copied(), Some(b'0'..=b'9'))
}

fn scan_lua_number(input: &str, start: usize) -> usize {
    let bytes = input.as_bytes();
    let mut index = start;

    if matches!(bytes.get(index).copied(), Some(b'.')) {
        index += 1;
        while matches!(bytes.get(index).copied(), Some(b'0'..=b'9' | b'_')) {
            index += 1;
        }
    } else if matches!(bytes.get(index).copied(), Some(b'0'))
        && matches!(bytes.get(index + 1).copied(), Some(b'x' | b'X'))
    {
        index += 2;
        while matches!(
            bytes.get(index).copied(),
            Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b'_')
        ) {
            index += 1;
        }
        if matches!(bytes.get(index).copied(), Some(b'.'))
            && !matches!(bytes.get(index + 1).copied(), Some(b'.'))
        {
            index += 1;
            while matches!(
                bytes.get(index).copied(),
                Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b'_')
            ) {
                index += 1;
            }
        }
        if matches!(bytes.get(index).copied(), Some(b'p' | b'P')) {
            index += 1;
            if matches!(bytes.get(index).copied(), Some(b'+' | b'-')) {
                index += 1;
            }
            while matches!(bytes.get(index).copied(), Some(b'0'..=b'9' | b'_')) {
                index += 1;
            }
        }
        return index;
    } else {
        while matches!(bytes.get(index).copied(), Some(b'0'..=b'9' | b'_')) {
            index += 1;
        }
    }

    if matches!(bytes.get(index).copied(), Some(b'.'))
        && !matches!(bytes.get(index + 1).copied(), Some(b'.'))
    {
        index += 1;
        while matches!(bytes.get(index).copied(), Some(b'0'..=b'9' | b'_')) {
            index += 1;
        }
    }

    if matches!(bytes.get(index).copied(), Some(b'e' | b'E')) {
        let exponent_start = index;
        index += 1;
        if matches!(bytes.get(index).copied(), Some(b'+' | b'-')) {
            index += 1;
        }
        let digits_start = index;
        while matches!(bytes.get(index).copied(), Some(b'0'..=b'9' | b'_')) {
            index += 1;
        }
        if digits_start == index {
            index = exponent_start;
        }
    }

    index
}

fn consume_lua_operator(input: &str, start: usize) -> usize {
    for token in ["...", "..", "==", "~=", "<=", ">=", "//", "<<", ">>", "::"] {
        if input[start..].starts_with(token) {
            return start + token.len();
        }
    }

    start
        + input[start..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1)
}
