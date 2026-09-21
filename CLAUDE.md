# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build --release          # Production build
cargo make check-format        # Format check (CI runs this)
cargo make clippy              # Lint — canonical blocking gate (== CI and bacon)
cargo make test                # Run all tests (== CI's nextest run)
cargo make clippy-jev          # Lint the jev decision engine (== clippy, + --features jev)
cargo make test-jev            # Run all tests with the jev feature enabled
```

Always prefer `cargo make <task>` over invoking `cargo clippy`/`cargo test` directly.
The `Makefile.toml` tasks are the canonical, `--locked` commands that CI
(`on-push.yml`, `on-merge.yml`'s `ci-flow`) and bacon (`cargo make monitor`) all
share byte-for-byte, so a clean local run predicts a clean CI run. Plain `cargo
clippy`/`cargo test` skip `--locked` and can silently pass locally on a resolution
CI would reject.

- `cargo make clippy` expands to `cargo clippy --all-targets --workspace --locked
  -- -D warnings` (see the `[tasks.clippy]` comment in `Makefile.toml`).
- `cargo make test` expands to `cargo nextest run --locked` — CI's `ci-flow` uses
  nextest, not the built-in test harness, so `cargo test` alone doesn't fully
  match CI.
- The `jev` decision engine lives behind the `jev` Cargo feature, off by
  default. It ships a second binary flavor, so it rots unless CI also builds,
  lints, and tests it: `cargo make clippy-jev` / `cargo make test-jev` are
  `clippy`/`test` plus `--features jev`, run as their own parallel CI jobs
  (`clippy-jev` in `on-push.yml`, `jev` — `cargo make ci-jev-flow` — in
  `on-merge.yml`) rather than serialized into the default-feature jobs.
  Default-feature artifacts and release packaging are unaffected by this
  feature.
