## What and why

<!-- One topic per PR. Link the issue: Closes #123 -->

## Tested on

<!-- Hardware and Windows build. For protocol work, paste the real frames you used. -->

## Checklist

- [ ] `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test --lib` pass
- [ ] New parsers have tests with real captured frames
- [ ] Device code stays read-only (no settings written)
- [ ] UI changes include a before/after screenshot
