# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

MultiTool is a progressive delivery CLI tool for canary deployments with automatic rollback. It helps teams catch production bugs before they impact users by managing traffic routing and monitoring deployments across AWS Lambda + API Gateway and Cloudflare Workers.

## Development Commands

### Building and Testing
- `cargo make test` - Run the full test suite using nextest
- `cargo make dev-test-flow` - Complete development workflow (format, clippy, build, docs, test)
- `cargo make ci-flow` - Full CI workflow including coverage
- `cargo make build` - Build the project
- `cargo make bacon` - Watch tests and rerun on file changes

### Code Quality
- `cargo make format` (alias: `fmt`) - Format code
- `cargo make check-format` - Check code formatting without changes
- `cargo make clippy-flow` - Run clippy lints

### Documentation and Utilities
- `cargo make gen-cli-reference` - Generate CLI reference docs in `target/debug/reference.md`
- `cargo make help` - Show help for the multi executable
- `cargo make wc` - Count lines of code using tokei
- `cargo make outdated` - Check for outdated dependencies

## Architecture

### Core Components

**Binary Target**: Single binary `multi` built from `src/bin/main.rs` that dispatches CLI commands.

**Subsystems Architecture**: The application uses an actor-based subsystem model with async communication:
- **Monitor Subsystem**: Reads observations from managed systems  
- **Ingress Subsystem**: Controls traffic routing to user services
- **Platform Subsystem**: Controls rollout of user services
- **Controller Subsystem**: Orchestrates other subsystems
- **Relay Subsystem**: Handles backend communication
- **Error Logs Subsystem**: Manages error logging (feature-gated)

**CLI Commands** (`src/cmd/`):
- `init` - Initialize new deployments
- `login`/`logout` - Authentication 
- `run` - Execute deployments
- `proxy` - Proxy functionality (feature-gated)
- `version` - Version information

### Key Modules

- **Adapters** (`src/adapters/`): Platform-specific implementations for Cloudflare, AWS Lambda, monitoring, and backends
- **Artifacts** (`src/artifacts/`): Loading and handling deployment artifacts (expects zipped Lambda functions)
- **Configuration** (`src/config/`): CLI configuration from environment and flags
- **Filesystem** (`src/fs/`): Filesystem abstraction respecting XDG_CONFIG
- **Stats** (`src/stats/`): Statistics library for monitoring and analysis
- **Metrics** (`src/metrics/`): Concrete metrics collection and observation
- **Terminal** (`src/terminal/`): Terminal communication with brand consistency

### Dependencies and Features

**Core Dependencies**: Built on tokio async runtime, uses clap for CLI, miette for error handling, serde for serialization.

**Feature Flags**:
- `proxy` - Enables pingora-based proxy functionality
- `errorlogs` - Enables error logging subsystem

**Test Framework**: Uses nextest for test execution with cargo-make for workflow orchestration.

## MultiTool API Integration

Default API endpoint: `https://api.multitool.run` (configurable via `MULTI_ORIGIN` environment variable)