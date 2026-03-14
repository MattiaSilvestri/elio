mod binary;
mod container;
mod directory;
mod dispatch;
mod document;
mod highlighting;
mod markdown;
mod structured;
mod text;
mod types;

#[cfg(test)]
pub(crate) use self::dispatch::build_preview;
pub(crate) use self::dispatch::{
    build_preview_with_options, loading_preview_for, should_build_preview_in_background,
};
use self::text::{
    collect_preview_lines, combine_preview_notes, count_source_lines, finalize_text_preview,
    read_text_preview, render_plain_text_preview, render_reflowed_text_preview,
    trim_trailing_line_endings, truncation_note,
};
use self::types::*;
pub(crate) use self::types::{
    PreviewContent, PreviewKind, PreviewRequestOptions, PreviewVisual, PreviewVisualKind,
    PreviewVisualLayout,
};

#[cfg(test)]
mod tests;
