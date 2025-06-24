# Build stage
FROM rust:1.87 as builder

# Install cargo-make for build process
RUN cargo install cargo-make

# Set working directory
WORKDIR /usr/src/multitool

# Copy workspace files
COPY Cargo.toml Cargo.lock Makefile.toml ./
COPY crates/ ./crates/

# Copy source code
COPY src/ ./src/

# Build the application using cargo make
RUN cargo make build

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -r -s /bin/false multitool

# Copy the binary from builder stage
COPY --from=builder /usr/src/multitool/target/debug/multi /usr/local/bin/multi

# Change ownership to non-root user
RUN chown multitool:multitool /usr/local/bin/multi

# Switch to non-root user
USER multitool

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/multi"]
