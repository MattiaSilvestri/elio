use super::*;
use crate::app::overlays::inline_image::{TerminalImageBackend, TerminalWindowSize};
use crate::preview::{
    PreviewContent, PreviewKind, PreviewVisual, PreviewVisualKind, PreviewVisualLayout,
};
use ratatui::layout::Rect;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_root(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("elio-preview-visual-{label}-{unique}"))
}

fn configure_terminal_image_support(app: &mut App) {
    let (cells_width, cells_height) = crossterm::terminal::size().unwrap_or((120, 40));
    app.terminal_images.backend = Some(TerminalImageBackend::KittyProtocol);
    app.terminal_images.window = Some(TerminalWindowSize {
        cells_width,
        cells_height,
        pixels_width: 1920,
        pixels_height: 1080,
    });
}

#[test]
fn preview_visual_overlay_request_uses_asset_metadata() {
    let root = temp_root("request-metadata");
    fs::create_dir_all(&root).expect("failed to create temp root");
    let asset_path = root.join("page.jpg");
    fs::write(&asset_path, b"jpeg").expect("failed to write image placeholder");

    let mut app = App::new_at(root.clone()).expect("app should initialize");
    configure_terminal_image_support(&mut app);
    app.entries = vec![Entry {
        path: root.join("book.cbz"),
        name: "book.cbz".to_string(),
        name_key: "book.cbz".to_string(),
        kind: EntryKind::File,
        size: 134 * 1024 * 1024,
        modified: None,
        readonly: false,
    }];
    app.selected = 0;
    app.frame_state.preview_media_area = Some(Rect {
        x: 2,
        y: 3,
        width: 48,
        height: 20,
    });
    app.preview_state.content = PreviewContent::new(PreviewKind::Archive, Vec::new())
        .with_preview_visual(PreviewVisual {
            kind: PreviewVisualKind::PageImage,
            layout: PreviewVisualLayout::Inline,
            path: asset_path.clone(),
            size: 11 * 1024,
            modified: None,
        });

    let request = app
        .active_preview_visual_overlay_request()
        .expect("preview visual overlay request should be available");

    assert_eq!(request.path, asset_path);
    assert_eq!(request.size, 11 * 1024);
    assert_eq!(request.modified, None);

    fs::remove_dir_all(root).expect("failed to remove temp root");
}

#[test]
fn page_image_visual_uses_full_preview_height() {
    let root = temp_root("full-height");
    fs::create_dir_all(&root).expect("failed to create temp root");

    let mut app = App::new_at(root.clone()).expect("app should initialize");
    configure_terminal_image_support(&mut app);
    app.preview_state.content = PreviewContent::new(PreviewKind::Archive, Vec::new())
        .with_preview_visual(PreviewVisual {
            kind: PreviewVisualKind::PageImage,
            layout: PreviewVisualLayout::FullHeight,
            path: root.join("page.jpg"),
            size: 11 * 1024,
            modified: None,
        });

    assert_eq!(
        app.preview_visual_rows(Rect {
            x: 0,
            y: 0,
            width: 48,
            height: 20,
        }),
        Some(20)
    );

    fs::remove_dir_all(root).expect("failed to remove temp root");
}

#[test]
fn inline_page_image_leaves_room_for_summary_text() {
    let root = temp_root("inline-page");
    fs::create_dir_all(&root).expect("failed to create temp root");

    let mut app = App::new_at(root.clone()).expect("app should initialize");
    configure_terminal_image_support(&mut app);
    app.preview_state.content = PreviewContent::new(PreviewKind::Comic, Vec::new())
        .with_preview_visual(PreviewVisual {
            kind: PreviewVisualKind::PageImage,
            layout: PreviewVisualLayout::Inline,
            path: root.join("page.jpg"),
            size: 11 * 1024,
            modified: None,
        });

    assert_eq!(
        app.preview_visual_rows(Rect {
            x: 0,
            y: 0,
            width: 48,
            height: 20,
        }),
        Some(14)
    );

    fs::remove_dir_all(root).expect("failed to remove temp root");
}
