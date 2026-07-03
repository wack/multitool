> **Self-validation suite for MultiTool Checks.** Each requirement below is a non-functional ("-ility") property of the `multi check` implementation, validated by a `prompt`-type check — instructions for an AI agent inspecting this repository. Place this file at the repository root and run `multi check` to dogfood the tool on itself.

# Requirement Provider-Agnostic Execution

The agent executor is defined behind a trait so execution can run over the in-process `cersei-agent` executor or a test fake without rewriting the execution phase. This extensibility is the whole point of the configuration/executor seam.

## Check Boxed Executor Trait

Inspect the Rust sources for the `multi check` feature. Confirm that agent execution is defined behind a trait (for example `CheckExecutor`) that is consumed as a boxed or `Arc`-wrapped, dynamically-dispatched trait object — for example a type alias of the form `Box<dyn CheckExecutor + Send + Sync>`, mirroring the existing `BoxedIngress`, `BoxedMonitor`, and `BoxedPlatform` aliases. The check passes only if the execution phase depends on this trait rather than naming a concrete executor type. Report a failure if the execution path references a concrete executor struct (such as the cersei executor) directly instead of the trait object.

# Requirement In-Process Agent Execution

The executor runs each check's agent **inside the CLI process** via the `cersei-agent` library — there is no requirement to shell out to any external CLI for a check to run.

## Check Default Executor Runs In-Process

Inspect the executor implementation and how it is constructed from configuration. Confirm that the execution engine builds and runs a `cersei_agent::Agent` in-process (calling its `run`/`run_stream` method) rather than spawning an external process. The check fails if running a check requires spawning an external `claude` process.

# Requirement Trustworthy In-Process Reporting

Check verdicts must arrive through a single in-process **judge tool**, `report-check-result`, registered on the agent — not from agent stdout, a sentinel file, or a process exit code. Capturing the verdict in-process is what makes results reliable despite agent nondeterminism.

## Check Verdict Captured Via Judge Tool

Inspect how a check's verdict is reported and captured. Confirm there is an in-process tool named `report-check-result` (implementing the cersei `Tool` trait) that is registered fresh on each check's agent and writes the verdict into an in-process sink the executor reads after the run. Confirm the verdict is **not** carried over a network MCP server or an external transport: there should be no in-process HTTP/`rmcp` server standing up per-check endpoints for reporting. The check fails if verdict reporting relies on an out-of-process server or a network endpoint.

## Check Verdict From Tool Call Not Stdout

Confirm that a check's pass/fail verdict is derived from the `success` boolean reported through the `report-check-result` judge tool, and never from the agent's stdout, stderr, or process exit code. Verify that a check whose agent finishes without ever calling the tool (for example by hitting the turn limit) is treated as a failure or error rather than silently passing. The check fails if the verdict is obtained by parsing stdout or by reading an exit status.

# Requirement Least-Privilege Agent Permissions

A verification agent should observe, not mutate. By default each check's agent gets a read-only tool set and a permission policy that denies anything beyond read-only, so a check cannot alter files even within its sandbox.

## Check Read-Only Tools By Default

Inspect how the in-process executor configures its agent's tools and permission policy. Confirm that, by default, the agent is given a read-only tool set (such as file-read, grep, and glob) plus the reporting/judge tool, and a read-only permission policy (for example `AllowReadOnly`) — not a full read-write-execute tool set with an allow-all policy. The check fails if the default agent is granted write or shell-execution tools, or an allow-everything permission policy.

# Requirement Isolated Agent Sessions

Checks run concurrently and the agent's shell tools persist per-session state in a process-global registry. Each check must therefore use a distinct session identifier so parallel agents cannot clobber one another's shell state.

## Check Distinct Session Per Check

Inspect how the in-process executor assigns a session identifier to each check's agent. Confirm that each check is given a unique `session_id` (for example derived from the check's id) rather than a shared constant, and that the per-session shell state is cleared on teardown. The check fails if all checks share one session id, or if per-check shell state is never cleared.

# Requirement Checks Cannot Corrupt the Workspace

Every check runs against a copy-on-write clone of the working tree, so a misbehaving agent cannot mutate the user's real files. Both checks below must pass for this requirement to be satisfied.

## Check Sandbox Is Cross-Platform and Platform-Gated

Inspect the sandboxing code. Confirm there is a sandbox abstraction (a trait such as `Sandbox`) with a working copy-on-write implementation on **both** macOS and Linux: macOS built on APFS `clonefile`, and Linux built on reflinks (the `FICLONE` ioctl) with a plain-copy fallback for filesystems that lack reflink support. Operating-system-specific code must be selected with `cfg` attributes so the crate still compiles on every target. The check fails if a real sandbox is created on only one of macOS or Linux (for example, if Linux falls through to a stub that returns an "unsupported platform" error instead of cloning the working tree), or if sandboxing is compiled unconditionally for a single operating system in a way that would break the build on other targets.

## Check Execution Uses the Sandbox

Confirm that the execution phase creates a sandbox for each check and runs the agent with the sandbox directory as its working directory, rather than executing the agent against the real working directory. The check fails if any check is executed outside of a sandbox.

# Req Concurrent Check Execution

(`Req` is the short alias for `Requirement`; this requirement uses it on purpose.) Checks are independent and each one spawns a potentially slow agent, so they must run concurrently rather than one after another.

## Check Concurrent Dispatch

Inspect the execution phase. Confirm that checks are dispatched concurrently — for example via a `JoinSet`, `FuturesUnordered`, or per-check `tokio::spawn` — and awaited together, rather than run inside a blocking loop that starts and awaits one check before beginning the next. The check fails if check execution is strictly sequential.

# Requirement Concurrency Defaults To The Host's Core Count

The concurrency cap is user-configurable (a `--concurrency` flag, layered the same way as `--provider`/`--model`/`--effort`), but absent any override its default must equal the number of CPU cores available on the machine running `multi check` — not a hardcoded constant. A fixed default either strands cores on big machines or overcommits small ones.

## Check Default Concurrency Equals Available Parallelism

Inspect how the default check concurrency is computed. Confirm that, with no `--concurrency` flag, no `MULTI_CHECKS_CONCURRENCY` environment variable, and no `checks.concurrency` config-file value set, the resolved concurrency is derived from the host's available parallelism (for example via `std::thread::available_parallelism`) rather than a fixed literal such as `2`. The check fails if the default concurrency is a hardcoded number instead of a value computed from the running machine's core count.

# Requirement Authoring Errors Are Actionable

When a `CHECKS.md` file is malformed, the tool must tell the author exactly what is wrong and where, instead of failing opaquely. Clear diagnostics are what make the format usable.

## Check Diagnostics Name the File

Confirm that discovery-time validation produces `miette` diagnostics for both malformed cases — a check with no associated requirement, and a requirement with neither an explicit check nor prose to promote into an anonymous check — and that each diagnostic identifies the offending `CHECKS.md` file. The check fails if either condition is unhandled, or if the diagnostic does not name the source file.

# Requirement CI-Friendly Exit Status

`multi check` must work as a CI gate: a clean pass exits zero, and any unsatisfied requirement exits non-zero.

## Check Exit Code Reflects Result

Confirm that the command exits with status code 0 when every requirement is satisfied (including the trivial case of an empty suite), and with status code 1 when one or more requirements are unsatisfied. The check fails if a run containing an unsatisfied requirement can exit 0, or if an all-satisfied run exits with a non-zero code.

# Requirement Layered Configuration

The model, provider, and effort are resolved from three sources with standard CLI precedence — flag, then environment, then config file (flag wins) — while credentials are read only from each provider's native environment variable, never from the config file.

## Check Precedence And Validation

Inspect the configuration phase of `multi check`. Confirm that provider/model/effort are merged from a config file, `MULTI_`-prefixed environment variables, and CLI flags, with flags overriding environment overriding file. Confirm the selected model is validated against a hardcoded allowlist of known IDs for the provider (an unknown ID is a clear error). The check fails if any of these values cannot be set from configuration, or if the merge precedence is not flag > env > file.

## Check Credentials Are Environment-Only

Confirm that provider API keys are read directly from each provider's native environment variable (for example `ANTHROPIC_API_KEY`) and are never loaded from the config file or from a `MULTI_`-prefixed variable. The check fails if a credential can be supplied through the config file or the `MULTI_` namespace.
