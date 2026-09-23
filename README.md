<img align="center" width="1200" alt="multitool-banner" src="https://github.com/user-attachments/assets/1463d3b1-ed84-4c8a-8374-abe0b53286b0" />

<h1 align="center">MultiTool Checks</h1>
<p align="center"><b>Enforce your specifications across every agent session.</b></p>

<p align="center">
🏡 <a href="https://www.multitool.run/">Home</a> • 📚 <a href="https://docs.multitool.run/">Docs</a>

## ❓ What is MultiTool Checks?

MultiTool Checks is a free, open-source CLI for verifying that a codebase satisfies your requirements. It runs like a test suite (invoked from the CLI, with a pass/fail exit code) but targets the non-functional "-ility" requirements that have no unit to test, like "authentication lives in one service" or "every public function is documented." You declare requirements in a `CHECKS.toml` file, each with one or more checks, and the tool evaluates every check with an AI agent in its own fresh context window, then reports a pass/fail conclusion for each.

We built this tool for internal use to stop multi-session drift, whereby your intent erodes across multiple agent sessions and earlier decisions are quietly forgotten downstream.

## 📖 Table of contents

* [Pre-requisites](#-pre-requisites)  
* [Install](#-pre-requisites)  
* [Get started](#-pre-requisites)  
* [The CHECKS.toml format](#-the-checkstoml-format)  
* [Roadmap](#%EF%B8%8F-roadmap)  
* [Mission](#-mission)  
* [Support](#-support)  
* [Privacy & license](#-privacy--license)

## 🧱 Pre-requisites

**macOS only** \- each check runs in a copy-on-write sandbox backed by APFS; Linux and Windows are not yet supported.

Checks are evaluated by a model through that provider's API; you bring your own key.

Supported providers:

* Anthropic (`ANTHROPIC_API_KEY`)  
* Gemini (`GEMINI_API_KEY`)  
* OpenAI (`OPENAI_API_KEY`)

## 🖥️ Install

With Homebrew:

```shell
brew install wack/tap/multi
```

Alternatively, visit [Releases](https://github.com/wack/multitool/releases/tag/v0.5.0) for the curl command and direct download links.

## ⭐ Get started

**1\. Create a `CHECKS.toml` at your repo root.** Each `[[requirement]]` declares a requirement, and each `[[requirement.check]]` beneath it declares one of its checks.

```toml
version = 1

[[requirement]]
id = "thin-handlers"
title = "Handlers contain no business logic"

  [[requirement.check]]
  id = "handlers-delegate"
  title = "Handlers delegate to the service layer"
  prompt = '''
  HTTP handlers delegate to a service layer. A handler that accesses the
  database or applies business rules directly fails this check.
  '''

[[requirement]]
id = "auth-in-keystore"
title = "Authentication lives in Keystore"
description = "Token issuance is centralized so credentials never sprawl across services."

  [[requirement.check]]
  id = "only-keystore-signs"
  title = "Only Keystore signs JWTs"
  prompt = "No service other than Keystore signs or issues JWTs."

  [[requirement.check]]
  id = "aviary-delegates"
  title = "Aviary delegates token issuance"
  prompt = "Aviary calls Keystore for token issuance rather than signing tokens itself."
```

**2\. Run the checks:**

```shell
multi check
```

`multi check` scans the working directory recursively for every `CHECKS.toml` (respecting `.gitignore`), or you can point it at a path: `multi check path/to/project`.

**3\. Read the report.** Passed requirements are shown in green and failed ones in red. Under each failure, the failing checks are listed with the agent's evidence so you can act on them, or hand them back to an agent to fix. Passing checks are omitted to keep the output focused.

```
✓ Handlers contain no business logic
✗ Authentication lives in Keystore
    ✗ Aviary delegates token issuance
      aviary signs its own session tokens (src/auth/session.rs:88)
```

A non-zero exit code on any failure makes it drop-in for CI.

## 📋 The CHECKS.toml format

* **`version = 1`** starts every file.  
* **`[[requirement]]`** declares a requirement: an `id` (any non-empty string, unique within the file), a `title`, and optional `description` and `tags`. The description is metadata only; it is never sent to the agent.  
* **`[[requirement.check]]`** declares a check belonging to the requirement above it: an `id` (unique within its requirement), a `title`, and a `prompt` handed to the agent. `kind = "prompt"` is the default and, for now, the only kind.  
* **Every requirement needs at least one check.** Unknown keys, duplicate ids, and a missing `prompt` are errors that point at the exact line.  
* **Ids are identity.** Plans (`.check-plan.toml`) are keyed by ids, so you can reorder requirements and checks, or retitle requirements, without losing cached results.  
* **Checks are ANDed.** A requirement passes only if all of its checks pass.  
* **Checks are independent.** Each runs in its own fresh context window, in no guaranteed order; a check should never assume another ran first.  
* **Keep each check narrow.** If a check needs the word "and," it is probably two checks.

## 🗺️ Roadmap

Up next:

* Linux and Windows support (macOS only today)  
* Shell-based checks for requirements better expressed as code  
* Executable checks that can run your project to verify behavior  
* Browser-based checks (Playwright) for frontend behavior

Longer term, we see MultiTool Checks as a first step toward spec-driven agentic development.

## 🎯 Mission

MultiTool Checks is built by [Wack](https://wack.run/), the team behind MultiTool. We want teams to get the full value of agentic development without needing to scale human oversight.

## 📬 Support

Found a bug or want to request a feature? [Open an issue](https://github.com/wack/multitool/issues/new) on the MultiTool repo and label it as a bug or feature request. For anything else, email support@wack.run.

## 🔐 Privacy & license

Anonymous by design: no account or login required, and checks run read-only. An agent inspects your repository but will never modify or execute it. Your code and checks stay local except for the content sent to your chosen provider's API to evaluate each check. Licensed under [`LICENSE`](https://github.com/wack/multitool/blob/trunk/LICENSE).
