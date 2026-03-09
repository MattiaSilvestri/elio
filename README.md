# elio

`elio` is a mouse-capable terminal file manager with a GNOME Files / Nautilus-inspired layout and a soft folder-first presentation.

## Features

- Nautilus-like shell with a top toolbar, places sidebar, main file area, and details pane
- Grid view by default, plus a denser list view
- Mouse click, double click, and wheel support
- Directory navigation, hidden-file toggle, sort cycling, refresh, and external open via `xdg-open`
- Lightweight text preview for readable files

## Run

```bash
cargo run
```

## Controls

- `Enter`: open the selected folder or file
- `Backspace`: go to the parent directory
- `Arrows` or `h/j/k/l`: navigate
- `v`: toggle grid/list view
- `.`: show or hide dotfiles
- `s`: cycle sort mode
- `r`: refresh the current directory
- `o`: open the selected file with `xdg-open`
- `?`: open the help overlay
- `q` or `Esc`: quit
