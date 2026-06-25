# Vendored `cersei-provider` (local patch)

This is a vendored copy of [`cersei-provider`](https://crates.io/crates/cersei-provider)
`0.1.9`, applied via `[patch.crates-io]` in the workspace `Cargo.toml`.

## Why

Upstream `0.1.9` (and `main` as of 2026-06) sends this header on **every**
Anthropic request, unconditionally:

```
anthropic-beta: interleaved-thinking-2025-04-14,token-efficient-tools-2025-02-19
```

The current Anthropic API rejects `interleaved-thinking-2025-04-14` with:

```
HTTP 400 invalid_request_error: Unexpected value(s) `interleaved-thinking-2025-04-14`
for the `anthropic-beta` header.
```

Because the header is not gated on whether thinking is enabled, this breaks
**every** request — making the in-process check executor (MULTI-1367) unusable
against Anthropic. There is no builder/config knob to disable it.

Upstream issue: https://github.com/pacifio/cersei/issues/20

## The change

`src/anthropic.rs`: `ANTHROPIC_BETA_HEADER` no longer includes the stale
`interleaved-thinking-2025-04-14` value. Only the still-accepted
`token-efficient-tools-2025-02-19` beta is sent. Extended thinking continues to
work through the `thinking` request-body parameter (which is GA and needs no beta
header). This is the single, localized diff from upstream `0.1.9`.

## Removing this patch

Delete `third_party/cersei-provider/`, drop the `[patch.crates-io]` block in the
workspace `Cargo.toml`, and bump `cersei-provider` to the first upstream release
that corrects (or makes configurable) the `anthropic-beta` header
(https://github.com/pacifio/cersei/issues/20).

## Related

Extended thinking is also left disabled in the check executor because
cersei-provider drops Anthropic thinking-block signatures off the stream
(`signature_delta`), so thinking blocks round-trip with an empty signature and
the API rejects them on the second turn. Tracked separately at
https://github.com/pacifio/cersei/issues/21; see `effort_temperature` in
`src/checks/executor/cersei.rs`.
