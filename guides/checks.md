# Validate requirements with `multi check`

`multi check` lets you declare **requirements** for a repository and have an
automated program validate that they hold. It's like a test suite — run it from
the CLI, get a pass/fail exit code — but it targets **non-functional ("ility")
requirements** that have no direct, programmatic unit to test (e.g. "no serif
fonts", "no images over 5 MB", "every public function is documented").

Each requirement is validated by one or more **checks**. In the MVP a check is a
`prompt`: a natural-language instruction that an AI agent carries out to decide
whether the requirement is satisfied. The agent runs **in-process** (no external
CLI), explores the sandbox with read-only tools, and reports its verdict. A
requirement is satisfied only if **all** of its checks pass (logical AND).

## ✅ Prerequisites

- [ ] An API key for your chosen provider in the environment (e.g.
      `ANTHROPIC_API_KEY`) — see [Configuration](#️-configuration). The
      in-process executor talks to the provider directly; **no `claude` CLI is
      required**.
- [ ] **macOS** — the MVP sandboxes each check with an APFS copy-on-write clone.
      Other operating systems are not yet supported.

## 🏃 Running it

From the root of your project:

```bash
multi check
```

`multi check` recursively scans the working directory for files named exactly
`CHECKS.toml`, runs every check it finds, and prints a report. You can point it at
a different directory:

```bash
multi check path/to/project
```

### Exit codes (for CI)

| Exit code | Meaning |
| --------- | ------- |
| `0`       | Every requirement is satisfied (an empty project — no `CHECKS.toml` — also exits `0`). |
| `1`       | One or more requirements are unsatisfied. |
| non-zero  | An operational error (e.g. a malformed `CHECKS.toml`), printed as a diagnostic. |

Because a clean failure is exit `1`, you can gate CI on it directly:

```bash
multi check || echo "requirements not met"
```

## 🗂️ The repository root

Each requirement is sandboxed at its own **repository root** — the nearest
directory, at or above its `CHECKS.toml`, that contains a MultiTool manifest
(`MultiTool.toml`, `.json`, or `.jsonc`). The agent that validates a check can
see everything under that root, and nothing outside it.

Root resolution happens **per requirements file**, not from the directory you
happen to invoke `multi check` from:

```bash
multi check                    # scans the whole tree
multi check services/keystore  # scans only that subtree
```

Both invocations discover the same `CHECKS.toml` files under the given
directory and give each of their requirements the *same* sandbox — the
`directory` argument only selects **which** requirements files run; it does
not shrink what their agents can see. A check's verdict is therefore stable
regardless of how the command was invoked.

**Monorepos**: a `MultiTool.toml` per service scopes that service's
requirements to just that service, even when `multi check` runs from the top
of the monorepo:

```
repo/
├── MultiTool.toml                 # repo-level manifest (optional)
└── services/
    ├── keystore/
    │   ├── MultiTool.toml         # keystore's own manifest
    │   └── CHECKS.toml            # sandboxed to services/keystore/
    └── metricstore/
        └── CHECKS.toml            # no manifest of its own — see below
```

`services/keystore/CHECKS.toml`'s requirements are sandboxed to
`services/keystore/` — the nearest manifest — while
`services/metricstore/CHECKS.toml` falls back to the rule below.

**No manifest anywhere above a `CHECKS.toml`**: its requirements fall back to
the directory `multi check` was scanned from. This is exactly the behavior
from before this rule existed, so manifest-less projects keep working
unchanged.

Because the sandbox can now span more than the directory a requirement
happens to live in, the agent's instructions also state where the requirement
was declared, as a path relative to the repository root (e.g. "This
requirement is declared in `services/keystore/CHECKS.toml`") — so the agent
still knows where to focus even inside a larger sandbox.

## ✍️ Authoring `CHECKS.toml`

A `CHECKS.toml` declares requirements and their checks as TOML tables. Every
file starts with the schema version:

```toml
version = 1
```

### Requirements

Each `[[requirement]]` table declares a requirement:

```toml
[[requirement]]
id = "no-yellow-text"
title = "No Yellow Text"
description = "I don't like the color yellow."
tags = ["style"]
```

| Key           | Required | Notes |
| ------------- | -------- | ----- |
| `id`          | yes      | Any non-empty string without leading/trailing whitespace or control characters, unique within the file. Matched exactly, so `"Auth"` and `"auth"` are different ids. |
| `title`       | yes      | Shown in the report. Titles need not be unique. |
| `description` | no       | Prose about the requirement. Metadata only; it is **never** sent to the agent. |
| `tags`        | no       | A list of strings for filtering and reporting. |

### Checks

Each `[[requirement.check]]` table declares a check belonging to the
requirement above it. Its `prompt` is the instruction handed to the agent; use a
`'''` literal string so Markdown and backslashes need no escaping:

```toml
[[requirement]]
id = "no-yellow-text"
title = "No Yellow Text"

  [[requirement.check]]
  id = "css-colors"
  title = "Confirm No Yellow Text"
  prompt = '''
  Scan each CSS file in this directory. For every rule that sets a `color`,
  ensure the value is not `yellow`.
  '''
```

| Key      | Required | Notes |
| -------- | -------- | ----- |
| `id`     | yes      | Same rules as a requirement id, unique within its requirement. |
| `title`  | yes      | Shown in the report. |
| `kind`   | no       | What kind of check this is. Defaults to `"prompt"`, currently the only kind. |
| `prompt` | yes, for `kind = "prompt"` | The agent's instructions. Leading and trailing whitespace is trimmed. |

Every requirement must declare at least one check.

### Multiple checks (ANDed)

A requirement may declare several checks. It is satisfied only if **all** of
them pass:

```toml
[[requirement]]
id = "small-images"
title = "Images must be under 5 MB"
description = "To keep downloads snappy, no image in this folder may exceed 5 MB."

  [[requirement.check]]
  id = "jpegs"
  title = "JPEGs"
  prompt = "List the `.jpg` files, `stat` each one, and flag any larger than 5 MB."

  [[requirement.check]]
  id = "svgs"
  title = "SVGs"
  prompt = "List the `.svg` files, `stat` each one, and flag any larger than 5 MB."
```

### Ids and plans

Ids, not titles or positions, identify requirements and checks. `multi plan`
keys each `.check-plan.toml` entry by `(requirement id, check id)`, so you can
reorder requirements and checks, or retitle requirements, without losing
their cached results. Changing an id, or a check's title or prompt,
invalidates that check's cached entry.

### Validation

Discovery rejects the whole suite, with a diagnostic naming the file and
pointing at the offending line, when a file:

- is not valid TOML, or has a missing or unsupported `version`;
- contains an unknown key (so a typo like `promt` is caught, not ignored);
- has a requirement with no checks, or a check with a missing or blank `prompt`;
- uses an unknown check `kind`;
- has an empty id, an id with leading/trailing whitespace or control
  characters, or a duplicate requirement or check id.

### Multiple files

You can keep more than one `CHECKS.toml` in a project — colocate requirements
with the code they describe, or split a long suite into pieces. Every
`CHECKS.toml` under the working directory is discovered recursively.
(`.gitignore` rules are respected, so generated and vendored trees are
skipped.) Ids only need to be unique within their own file.

## 📊 How results are reported

Each requirement's title is printed in **green** if it passed and **red** if it
failed. For a failed requirement, its **failing checks** are listed in red along
with the agent's *evidence* explaining why. Passing checks are omitted to keep
the output focused. Color follows the global `--enable-colors` flag and degrades
to a clear plain-text form when disabled.

## 🔒 The trust model

Agents are nondeterministic, so `multi check` does **not** trust their stdout or
any sentinel file. Instead, each agent reports its verdict by calling a single
in-process **judge tool**, `report-check-result(success, evidence?)`, registered
fresh on that check's agent and closing over its own result sink. An agent that
finishes **without** calling the tool fails its check. This keeps results
trustworthy despite agent nondeterminism.

Agents run with a **least-privilege, read-only** tool set by default (Read, Grep,
Glob, plus the judge tool) — a verification agent observes, it does not mutate.

## ⚙️ Configuration

The default **provider**, **model**, **effort**, and **concurrency** are
resolved from three sources, in order of precedence (highest wins):

1. **Flags** — `--provider`, `--model`, `--effort`, `--concurrency` on
   `multi check`.
2. **Environment** — `MULTI_`-prefixed vars mapped into the `checks` namespace,
   e.g. `MULTI_CHECKS_MODEL`, `MULTI_CHECKS_PROVIDER`, `MULTI_CHECKS_EFFORT`,
   `MULTI_CHECKS_CONCURRENCY`.
3. **Config file** — the `[checks]` table of `MultiTool.toml` (or `.json` /
   `.jsonc`), discovered up the directory tree like any MultiTool manifest.

```toml
[checks]
provider    = "anthropic"          # anthropic | openai | gemini
model       = "claude-sonnet-4-6"  # must be a known model ID for the provider
effort      = "low"                # low | medium | high  → thinking-token budget
concurrency = 8                    # checks run at once; must be > 0 (default: CPU core count)

# optional, non-secret base-URL overrides per provider
[checks.providers.anthropic]
base_url = "https://..."
```

An unset flag contributes nothing — it never overrides a value from the
environment or file. The `model` is validated against a hardcoded allowlist of
known IDs for the selected provider; an unknown ID is a clear error. `effort`
maps to the in-process agent's extended-thinking budget: `medium` and `high`
enable extended thinking (4096- and 8192-token budgets respectively), while
`low` — the default — keeps thinking off for speed and cost, running the agent
deterministically instead.

Each check runs as an in-process agent (native multi-provider model swapping,
no external CLI or subprocess).

The **`concurrency`** flag caps how many checks run at once; it must be a
positive integer (`0` is rejected with a clear error). Its default matches the
number of CPU cores available on the machine running `multi check`, so a suite
fans out to use the whole machine rather than leaving cores idle.

**Credentials are environment-only.** API keys are read directly from each
provider's native variable and never live in the config file or under the
`MULTI_` prefix:

| Provider  | API key                                  | Base URL (optional)  |
| --------- | ---------------------------------------- | -------------------- |
| Anthropic | `ANTHROPIC_API_KEY`                      | `ANTHROPIC_BASE_URL` |
| OpenAI    | `OPENAI_API_KEY`                         | `OPENAI_BASE_URL`    |
| Gemini    | `GOOGLE_API_KEY` (or `GEMINI_API_KEY`)   | `GEMINI_BASE_URL`    |

A provider is only selectable when its API key is present; selecting a provider
whose key is missing is an error.

### Routing through Fireworks

Fireworks exposes an Anthropic-compatible Messages endpoint, so it slots into
the `anthropic` provider rather than needing its own provider kind: override
`base_url` to Fireworks and set `ANTHROPIC_API_KEY` to a Fireworks key
(`fw_...`). A repo-root `MultiTool.toml` doing this:

```toml
[checks]
provider = "anthropic"
model    = "accounts/fireworks/routers/glm-5p1-fast"

[checks.providers.anthropic]
base_url = "https://api.fireworks.ai/inference"
```

The `model` must still be a known ID in `ANTHROPIC_MODELS`
(`src/checks/config/models.rs`) — add the Fireworks model ID you want there
before pointing `base_url` at it.

## ⚠️ MVP constraints

- **macOS only** — copy-on-write sandboxing uses APFS `clonefile`. Linux and
  Windows support is planned.
- **`prompt`-type checks only** — checks run an in-process agent against the
  configured model (the `sonnet` family by default). A `shell` check type is
  planned.
- **Read-only agents** — checks observe the sandbox with read-only tools and
  cannot execute code. Per-check execution capability (for checks that must run
  the project to verify behavior) is planned.

## 📬 Need help?

If you have questions, ideas, or bugs to report:

👉 [support@multitool.run](mailto:support@multitool.run)
