# Session handover — storybloq setup for MultiTool Checks

## What this project is

`.story/` was initialized for the **multitool** repo (Rust / cargo CLI, the `multi`
binary; mature v0.4.0 canary-deployment tool). The roadmap tracks the **MultiTool
Checks** feature — a new `multi check` subcommand that validates declared
non-functional ("ility") requirements by running AI-agent checks inside copy-on-write
sandboxes and collecting verdicts through an in-process `rmcp` MCP server. Active
branch: `robbie/mt-check`. The PRD lives at `prompt.md` (gitignored scratch file).

## How this roadmap was created

This storybloq roadmap is a **one-to-one mirror of the existing Linear project
"MultiTool Checks"** (team MULTI, project id 781c5b95-…). It was NOT independently
designed — Robbie had already authored the Linear project + tickets from `prompt.md`
and asked to mirror them.

- **Linear milestones → storybloq phases** (9): M0 · Subcommand skeleton, M1 ·
  Discovery, M2 · Configuration & executor, M3 · Sandboxing, M4 · MCP result server,
  M5 · Execution, M6 · Reporting & exit, Tests & docs, Future work (post-MVP).
- **Linear issues → storybloq tickets** (34, one-to-one): MULTI-1331..MULTI-1364.
  Created in Linear-numeric order so T-001..T-034 line up with the issue order.
- **Each ticket's description carries its Linear ID, URL, and `gitBranchName`**
  (`robbie/multi-XXXX`, verbatim from the Linear API). Pushing that branch to GitHub
  auto-links the Linear issue. The branch is the linkage mechanism — storybloq has no
  dedicated Linear field, so it lives in the description footer.
- **Epic preserved**: MULTI-1332 (the in-process MCP server) = **T-013**, a `feature`;
  its 5 Linear sub-issues (MULTI-1344..1348) = T-014..T-018, set as storybloq
  sub-tickets (`parentTicket: T-013`).

## Ticket-number map (storybloq ↔ Linear)

T-001→1331 · T-002→1333 · T-003→1334 · T-004→1335 · T-005→1336 · T-006→1337 ·
T-007→1338 · T-008→1339 · T-009→1340 · T-010→1341 · T-011→1342 · T-012→1343 ·
T-013→1332(epic) · T-014→1344 · T-015→1345 · T-016→1346 · T-017→1347 · T-018→1348 ·
T-019→1349 · T-020→1350 · T-021→1351 · T-022→1352 · T-023→1353 · T-024→1354 ·
T-025→1355 · T-026→1356 · T-027→1357 · T-028→1358 · T-029→1359 · T-030→1360 ·
T-031→1361 · T-032→1362 · T-033→1363 · T-034→1364.
(Stable map is the Linear ID in each ticket's description, not the T-number.)

## Type mapping convention

Linear labels don't map cleanly to storybloq's task/feature/chore, so: MVP
implementation issues → `task`; the MCP epic (T-013) and all 8 post-MVP "Future
work" items → `feature`; the two test issues + the docs issue → `chore`.

## Decisions captured this session

- **Markdown parser = `comrak`** (NOT pulldown-cmark), per Robbie. Baked into T-004's
  description AND written back to Linear MULTI-1335 (its description previously left
  comrak/pulldown-cmark as open "candidates"; now a firm Decision section).

## Config

- `recipeOverrides.stages`: TEST = `cargo nextest run --workspace`,
  BUILD = `cargo build --workspace`. WRITE_TESTS/VERIFY left off (CLI, no dev server).
- `.gitignore`: added `.story/snapshots/`, `.story/sessions/`, `.story/status.json`
  (roadmap + tickets are tracked).

## State / next steps

- All 34 tickets are `open`; nothing started. Implementation order follows the phases
  (M0 → M6, then Tests & docs; Future work is post-MVP backlog).
- Natural first ticket: **T-001** (wire up `multi check` subcommand + phase skeleton),
  which everything else builds on.
- If new Linear issues are added to the project later, mirror them the same way
  (one ticket, Linear ID + branch in the description, correct phase).
