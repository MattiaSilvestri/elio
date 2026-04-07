# Contributing

## Toolchain

`rust-toolchain.toml` pins the project to Rust 1.93.0 and installs the required `rustfmt` and `clippy` components automatically when you work in the repository.

## Local Quality Checks

Before opening a PR, run the CI checks locally:

```bash
cargo fmt --check
cargo test --locked --test architecture_guardrails
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
```

## Security Auditing

Run `cargo audit --deny unsound` to check for security advisories.

Three `unmaintained` warnings are expected and known:

- **RUSTSEC-2025-0141** — `bincode` via `syntect` (latest): upstream-bound, no fix available
- **RUSTSEC-2024-0320** — `yaml-rust` via `syntect` (latest): upstream-bound, no fix available
- **RUSTSEC-2024-0436** — `paste` via `lofty` (latest): upstream-bound, no fix available

These are not actionable. The command fails only on `unsound` advisories, which are a hard blocker.

## Optional Preview Tooling

For the broadest local preview-test coverage, install the optional archive and PDF tools used by the test suite, especially `7z`, `bsdtar`, `isoinfo`, `pdfinfo`, `pdftocairo`, and `xz`.
