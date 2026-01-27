# Support setting various labels on the final image
ARG COMMIT=""
ARG VERSION=""
ARG BUILDNUM=""

FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

LABEL org.opencontainers.image.source="https://github.com/loadnetwork/load-reth"
LABEL org.opencontainers.image.description="Load-Reth execution client for Load Network"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"

# Install system dependencies
RUN apt-get update && apt-get -y upgrade && apt-get install -y --no-install-recommends libclang-dev pkg-config && \
    rm -rf /var/lib/apt/lists/*

# Builds a cargo-chef plan. .dockerignore already filters git/target artifacts.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Build profile, release by default
ARG BUILD_PROFILE=release
ENV BUILD_PROFILE=$BUILD_PROFILE

# Extra Cargo flags for optimization and multi-platform support
ARG RUSTFLAGS=""
ARG TARGETPLATFORM
ENV RUSTFLAGS="$RUSTFLAGS"

# Extra Cargo features (jemalloc + asm-keccak for performance)
ARG FEATURES="jemalloc,asm-keccak"
ENV FEATURES=$FEATURES

# Configure target architecture based on platform
RUN case "$TARGETPLATFORM" in \
    "linux/amd64") echo "x86_64-unknown-linux-gnu" > /tmp/target.txt ;; \
    "linux/arm64") echo "aarch64-unknown-linux-gnu" > /tmp/target.txt ;; \
    "linux/arm/v7") echo "armv7-unknown-linux-gnueabihf" > /tmp/target.txt ;; \
    *) echo "x86_64-unknown-linux-gnu" > /tmp/target.txt ;; \
    esac

# Install target for cross-compilation if needed
RUN TARGET=$(cat /tmp/target.txt) && \
    if [ "$TARGET" != "x86_64-unknown-linux-gnu" ]; then \
        rustup target add $TARGET; \
    fi

# Builds dependencies
RUN TARGET=$(cat /tmp/target.txt) && \
    cargo chef cook --locked --profile $BUILD_PROFILE --features "$FEATURES" --target $TARGET --recipe-path recipe.json

# Build application
COPY . .
RUN TARGET=$(cat /tmp/target.txt) && \
    cargo build --profile $BUILD_PROFILE --features "$FEATURES" --target $TARGET --locked --bin load-reth

# ARG is not resolved in COPY so we have to hack around it by copying the
# binary to a temporary location
RUN TARGET=$(cat /tmp/target.txt) && \
    cp /app/target/$TARGET/$BUILD_PROFILE/load-reth /app/load-reth

# Use Ubuntu 24.04 as the release image (modern, LTS, security updates)
FROM ubuntu:24.04 AS runtime

# Create non-root user for security (fixed UID/GID for host volume ownership)
ARG RETH_UID=10001
ARG RETH_GID=10001
RUN groupadd -r -g "${RETH_GID}" reth && useradd -r -u "${RETH_UID}" -g reth -d /home/reth -m reth

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

# Copy load-reth over from the build stage
COPY --from=builder /app/load-reth /usr/local/bin/
RUN chmod +x /usr/local/bin/load-reth

# Copy licenses (if they exist) - use RUN since COPY doesn't support shell redirects
RUN mkdir -p /licenses

# Create data directory and set ownership
RUN mkdir -p /data && chown -R reth:reth /data

# Switch to non-root user
USER reth

# Set default environment
ENV RUST_LOG=info

# Expose standard Ethereum execution client ports
# 30303: P2P TCP/UDP
# 8545: HTTP JSON-RPC
# 8546: WebSocket JSON-RPC
# 8551: Engine API (authenticated)
# 9001: Metrics
EXPOSE 30303 30303/udp 8545 8546 8551 9001

# Data volume
VOLUME ["/data"]

# Basic health check that only verifies the load-reth process is running so it
# does not depend on HTTP/Engine flags.
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD grep -q load-reth /proc/1/comm || exit 1

# Add metadata labels to help programmatic image consumption
ARG COMMIT=""
ARG VERSION=""
ARG BUILDNUM=""
LABEL commit="$COMMIT" version="$VERSION" buildnum="$BUILDNUM"

ENTRYPOINT ["/usr/local/bin/load-reth"]
