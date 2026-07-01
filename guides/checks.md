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
      `ANTHROPIC_API_KEY`) — see [Configuration](#️-configuration). The default
      in-process executor talks to the provider directly; **no `claude` CLI is
      required**. (The optional `claude -p` fallback — `--executor claude` — does
      need the [`claude`](https://docs.claude.com/en/docs/claude-code) CLI on your
      `PATH`.)
- [ ] **macOS** — the MVP sandboxes each check with an APFS copy-on-write clone.
      Other operating systems are not yet supported.

## 🏃 Running it

From the root of your project:

```bash
multi check
```

`multi check` recursively scans the working directory for files named exactly
`CHECKS.md`, runs every check it finds, and prints a report. You can point it at
a different directory:

```bash
multi check path/to/project
```

### Exit codes (for CI)

| Exit code | Meaning |
| --------- | ------- |
| `0`       | Every requirement is satisfied (an empty project — no `CHECKS.md` — also exits `0`). |
| `1`       | One or more requirements are unsatisfied. |
| non-zero  | An operational error (e.g. a malformed `CHECKS.md`), printed as a diagnostic. |

Because a clean failure is exit `1`, you can gate CI on it directly:

```bash
multi check || echo "requirements not met"
```

## ✍️ Authoring `CHECKS.md`

`CHECKS.md` files are ordinary Markdown. Two header patterns carry metadata;
everything else is prose.

### Requirements

An **H1** whose text begins with `Requirement ` (or the alias `Req `) declares a
requirement. The rest of the line is its **title**.

```markdown
# Requirement No Yellow Text
```

```markdown
# Req No Yellow Text
```

Both declare a requirement titled `No Yellow Text`.

### Checks

An **H2** whose text begins with `Check ` declares a check. The text after
`Check ` is the check's title, and the Markdown **beneath** it (up to the next
requirement or check) is the **prompt** handed to the agent. A check belongs to
the nearest requirement above it.

```markdown
# Requirement No Yellow Text
I don't like the color yellow.

## Check Confirm No Yellow Text
Scan each CSS file in this directory. For every rule that sets a `color`,
ensure the value is not `yellow`.
```

Here, `I don't like the color yellow.` is **prose** — optional metadata that is
ignored when a requirement has explicit `## Check` headers.

### Anonymous checks (prose-as-check)

If a requirement declares **no** `## Check`, its prose body becomes a single
**anonymous check** that **inherits the requirement's title**:

```markdown
# Requirement No Serif Fonts
Scan the CSS files in this directory. Check each font.
Ensure none of the named fonts are serif fonts.
```

This is equivalent to one requirement `No Serif Fonts` with one check, also
titled `No Serif Fonts`, whose prompt is the prose above.

A requirement with **neither** a `## Check` **nor** prose is an error, as is a
`## Check` with no requirement above it.

### Multiple checks (ANDed)

A requirement may declare several checks. It is satisfied only if **all** of
them pass:

```markdown
# Requirement Images must be under 5 MB
To keep downloads snappy, no image in this folder may exceed 5 MB.

## Check JPEGs
List the `.jpg` files with `ls`/`grep`, `stat` each one, and flag any larger
than 5 MB.

## Check SVGs
List the `.svg` files with `ls`/`grep`, `stat` each one, and flag any larger
than 5 MB.
```

### Multiple files

You can keep more than one `CHECKS.md` in a project — colocate requirements with
the code they describe, or split a long suite into pieces. Every `CHECKS.md`
under the working directory is discovered recursively. (`.gitignore` rules are
respected, so generated and vendored trees are skipped.)

Titles are **not** unique — two requirements (or two checks) may share a title,
and both are kept.

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

The default **provider**, **model**, **effort**, **executor**, and
**concurrency** are resolved from three sources, in order of precedence
(highest wins):

1. **Flags** — `--provider`, `--model`, `--effort`, `--executor`,
   `--concurrency` on `multi check`.
2. **Environment** — `MULTI_`-prefixed vars mapped into the `checks` namespace,
   e.g. `MULTI_CHECKS_MODEL`, `MULTI_CHECKS_PROVIDER`, `MULTI_CHECKS_EFFORT`,
   `MULTI_CHECKS_EXECUTOR`, `MULTI_CHECKS_CONCURRENCY`.
3. **Config file** — the `[checks]` table of `MultiTool.toml` (or `.json` /
   `.jsonc`), discovered up the directory tree like any MultiTool manifest.

```toml
[checks]
provider    = "anthropic"          # anthropic | openai | gemini
model       = "claude-sonnet-4-6"  # must be a known model ID for the provider
effort      = "low"                # low | medium | high  → thinking-token budget
executor    = "cersei"             # cersei (in-process, default) | claude (fallback)
concurrency = 8                    # checks run at once; must be > 0 (default: CPU core count)

# optional, non-secret base-URL overrides per provider
[checks.providers.anthropic]
base_url = "https://..."
```

An unset flag contributes nothing — it never overrides a value from the
environment or file. The `model` is validated against a hardcoded allowlist of
known IDs for the selected provider; an unknown ID is a clear error. `effort`
currently maps to the in-process agent's sampling temperature (`low` → most
deterministic, `high` → most exploratory); mapping it to an extended-thinking
budget is pending an upstream provider fix.

The **`executor`** selects the execution engine. The default `cersei` runs each
check as an in-process agent (native multi-provider model swapping, no external
CLI). `claude` is the legacy `claude -p` shell-out fallback, kept selectable for
migration while the in-process path is validated; it requires the `claude` CLI on
your `PATH` and will be removed once cersei is proven out.

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
