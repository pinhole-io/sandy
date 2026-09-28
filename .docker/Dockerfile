# Build stage
FROM rustlang/rust:nightly AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    musl-tools \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /build

# Copy Cargo manifest and sources
# Build context must be the sandy/ directory:
#   docker build -f .docker/Dockerfile -t sandy .
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# Build the binary from workspace root with musl target for Alpine compatibility
# Using BuildKit cache mounts for faster rebuilds
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    rustup target add x86_64-unknown-linux-musl && \
    cargo build --release --target x86_64-unknown-linux-musl -p sandy && \
    cp /build/target/x86_64-unknown-linux-musl/release/sandy /build/sandy && \
    test -f /build/sandy || (echo "Binary not found" && find /build/target/x86_64-unknown-linux-musl/release -name "*sandy*" -type f && exit 1)

# Runtime stage
FROM alpine:latest

# Build argument for Python version (default: 3.13 — uv publishes musl builds; 3.14+ may not yet)
ARG PYTHON_VERSION=3.13
# Pin uv release (see https://github.com/astral-sh/uv/releases)
ARG UV_VERSION=0.6.14

# Install runtime dependencies
RUN apk add --no-cache \
    ca-certificates \
    curl \
    bash

# Unprivileged user for sandboxed execution (must match DockerConfig.sandbox.container_user)
RUN adduser -D -u 1000 -h /home/sandbox sandbox && \
    mkdir -p /workspace /home/sandbox && \
    chown -R sandbox:sandbox /home/sandbox /workspace

# Install uv to a system path (installer defaults to ~/.local/bin)
RUN curl -fsSL "https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-installer.sh" -o /tmp/uv-installer.sh && \
    sh /tmp/uv-installer.sh && \
    install -m 755 /root/.local/bin/uv /usr/local/bin/uv && \
    rm -f /tmp/uv-installer.sh

ENV PATH="/usr/local/bin:$PATH"

# Verify uv installation
RUN uv --version

# Node.js + npm (Alpine package; used by `sandy exec-js` / `docker-exec-js`)
RUN apk add --no-cache nodejs npm && node --version && npm --version

# Pre-install Python under /opt (not /workspace or /tmp: those are tmpfs at runtime and hide image layers).
ENV UV_PYTHON_INSTALL_DIR=/opt/uv-python
RUN mkdir -p /opt/uv-python && uv python install "${PYTHON_VERSION}" && chmod -R a+rX /opt/uv-python
ENV UV_PYTHON="${PYTHON_VERSION}"

# Copy the binary from builder (copied to non-cached location)
COPY --from=builder /build/sandy /usr/local/bin/sandy
RUN chmod 755 /usr/local/bin/sandy

# Default runtime user (also enforced via Docker API HostConfig / Config.user)
ENV HOME=/workspace
ENV SANDY_IN_DOCKER=1
USER sandbox

# Set working directory for Python scripts
WORKDIR /workspace

# Default entrypoint
ENTRYPOINT ["sandy"]

# Example usage:
# IMPORTANT: The build context must be the sandy/ directory, not the workspace root!
#
# From the sandy/ directory:
#   docker build -f .docker/Dockerfile -t sandy .
#
# Run: docker run --rm sandy exec-code --code 'print("Hello from container!")'
