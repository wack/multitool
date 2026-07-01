# Bugs

These are bugs (or missing features) I've observed while working with `multi checks`.

- [ ] No use of Cersei workflows to chain multiple prompts together.

- [ ] No support for Fireworks AI.

- [ ] No GitHub Action available.

- [ ] No loading of skills.

- [ ] No loading of RULES.md files from the .claude directory.

- [ ] Assemble_instructions is hard-coded: src/checks/executor/mod.rs:98 (definition), called from src/checks/executor/cersei.rs:110

- [ ] No system prompt provided.

- [ ] Not sure if prompt caching is enabled at all.

- [ ] No trace capture. We need a way to record all session traces so that we can analyze why they failed.

- CERSEI: `append_system_prompt()` function is dead unless routed through the separate build_system_prompt() composer.

## Fixes

- [x] No loading of CLAUDE.md files

- [x] Concurrency still not respected.

- [x] Full error text got cut off at the end of the terminal screen instead of wrapping. Turned out to
live in the *presenter* (`src/checks/presenter/inline.rs`), not the `Reporter` — the inline TUI is the
sole terminal writer for the whole run (see `owns_record` in `src/checks/mod.rs`), so `Reporter::report()`
never even runs in a TTY session. The presenter renders into a fixed-size `ratatui::Buffer` via
`insert_before`, which clips instead of wrapping; fixed by word-wrapping every flushed line to the
terminal width before building it.

- [x] A `tracing::info!`/`debug!` log line fired mid-run (e.g. the "retrying check whose agent did not
report" line) wrote raw bytes straight to stdout, corrupting the inline TUI's cursor-managed viewport.
Fixed by giving the presenter full ownership of log output: `PresenterActor` now registers itself as
`tracing`'s active sink (`src/terminal/logging.rs::route_logs`) for the run's duration and folds each
line in as a `UiEvent::Log`. The inline TUI flushes each line to permanent scrollback (same mechanism as
a completed requirement) and also keeps the last few in a small live pane, separate from the tree.
