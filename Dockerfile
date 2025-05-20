# This Dockerfile defines a development environment for working with
# MultiTool. It's meant to serve as a jumping off point for contributors
# who may not have the right tools installed for development.

FROM rust:1.86.0-slim-bookworm

RUN apt-get update && \
    apt-get install libssl-dev pkg-config -yq && \
    cargo install cargo-make cargo-nextest
