> **Self-validation suite for MultiTool Checks.** Each requirement below is a non-functional ("-ility") property of the `multi check` implementation, validated by a `prompt`-type check — instructions for an AI agent inspecting this repository. Place this file at the repository root and run `multi check` to dogfood the tool on itself.

# Requirement Provider-Agnostic Execution

The agent executor must be swappable so we can evolve from shelling out to `claude -p` toward a Claude Code SDK (or another provider) without rewriting the execution phase. This extensibility is the whole point of the configuration/executor seam.

## Check Boxed Executor Trait

Inspect the Rust sources for the `multi check` feature. Confirm that agent execution is defined behind a trait (for example `CheckExecutor`) that is consumed as a boxed, dynamically-dispatched trait object — a type alias of the form `Box<dyn CheckExecutor + Send + Sync>`, mirroring the existing `BoxedIngress`, `BoxedMonitor`, and `BoxedPlatform` aliases. The check passes only if the execution phase depends on this trait rather than naming a concrete `claude -p` executor type. Report a failure if the execution path references a concrete executor struct directly instead of the trait object.

# Requirement Trustworthy In-Process Reporting

Check verdicts must arrive through the single `report-check-result` MCP tool, served from inside the CLI process. This guardrail is what makes results reliable despite agent nondeterminism; running the server out-of-process, or trusting agent stdout, would defeat it.

## Check Server Runs In-Process

Inspect how the result-reporting MCP server is started. Confirm it is built with the `rmcp` framework and run on a Tokio task within the CLI process — not spawned as a child process. Search for any use of `std::process::Command` or `tokio::process` that would launch the server externally; if the MCP server runs as a subprocess, the check fails. It passes only if the server runs on a task in the same process as the CLI.

## Check Verdict From Tool Call

Confirm that a check's pass/fail verdict is derived from the `success` boolean of the `report-check-result` tool call, and never from the agent's stdout, stderr, or process exit code. Verify that a check whose agent exits without ever calling the tool is treated as a failure or error rather than silently passing. The check fails if the verdict is obtained by parsing stdout or by reading the agent's exit status.

# Requirement Stateful MCP Sessions

The Claude Code MCP client connects to the result-reporting server over Streamable HTTP and expects the standard *stateful* session flow: it sends `initialize`, receives an `Mcp-Session-Id`, and issues subsequent requests under that session. A stateless server stalls this multi-step handshake, so the in-process MCP server must run in stateful mode.

## Check Server Is Stateful

Inspect how the `rmcp` result-reporting MCP server is configured (its `StreamableHttpServerConfig`). Confirm the server runs in stateful Streamable HTTP mode — that is, `stateful_mode` is true and is not set to `false`. The check passes only if the server is configured for stateful sessions; it fails if `stateful_mode` is set to `false` (stateless mode).

# Requirement Checks Cannot Corrupt the Workspace

Every check runs against a copy-on-write clone of the working tree, so a misbehaving agent cannot mutate the user's real files. Both checks below must pass for this requirement to be satisfied.

## Check Sandbox Exists and Is Platform-Gated

Inspect the sandboxing code. Confirm there is a sandbox abstraction (a trait such as `Sandbox`) with a macOS copy-on-write implementation built on APFS `clonefile`, and that operating-system-specific code is selected with `cfg` attributes so the crate still compiles on non-macOS targets. The check fails if sandboxing is compiled unconditionally for a single operating system in a way that would break the build on other targets.

## Check Execution Uses the Sandbox

Confirm that the execution phase creates a sandbox for each check and runs the agent with the sandbox directory as its working directory, rather than executing the agent against the real working directory. The check fails if any check is executed outside of a sandbox.

# Req Concurrent Check Execution

(`Req` is the short alias for `Requirement`; this requirement uses it on purpose.) Checks are independent and each one spawns a potentially slow agent, so they must run concurrently rather than one after another.

## Check Concurrent Dispatch

Inspect the execution phase. Confirm that checks are dispatched concurrently — for example via a `JoinSet`, `FuturesUnordered`, or per-check `tokio::spawn` — and awaited together, rather than run inside a blocking loop that starts and awaits one check before beginning the next. The check fails if check execution is strictly sequential.

# Requirement Authoring Errors Are Actionable

When a `CHECKS.md` file is malformed, the tool must tell the author exactly what is wrong and where, instead of failing opaquely. Clear diagnostics are what make the format usable.

## Check Diagnostics Name the File

Confirm that discovery-time validation produces `miette` diagnostics for both malformed cases — a check with no associated requirement, and a requirement with neither an explicit check nor prose to promote into an anonymous check — and that each diagnostic identifies the offending `CHECKS.md` file. The check fails if either condition is unhandled, or if the diagnostic does not name the source file.

# Requirement CI-Friendly Exit Status

`multi check` must work as a CI gate: a clean pass exits zero, and any unsatisfied requirement exits non-zero.

## Check Exit Code Reflects Result

Confirm that the command exits with status code 0 when every requirement is satisfied (including the trivial case of an empty suite), and with status code 1 when one or more requirements are unsatisfied. The check fails if a run containing an unsatisfied requirement can exit 0, or if an all-satisfied run exits with a non-zero code.

# Requirement No Hidden Configuration

For the MVP, the model, model-provider URL, and effort level are hardcoded and injected into the pipeline. Inspect the configuration phase of `multi check` and confirm these values are hardcoded, and that the discovery and execution phases do not read them from environment variables or from a configuration file. The requirement is satisfied only if configuration is hardcoded for the MVP; it is not satisfied if any model, provider, or effort value is sourced from the environment or a configuration file.
