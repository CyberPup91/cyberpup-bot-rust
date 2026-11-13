# --- Builder Stage ---
FROM rust:1.89-slim AS builder

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    g++ \
    pkg-config \
    libopus-dev \
    libssl-dev \
    libfontconfig1-dev \
    libfreetype6-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create temporary source files so Cargo can resolve and build dependencies
RUN mkdir -p src/commands && \
    echo "pub struct Data {} pub type Error = Box<dyn std::error::Error + Send + Sync>; pub type Context<'a> = poise::Context<'a, Data, Error>; fn main() {}" > src/main.rs && \
    echo "pub fn all() -> Vec<poise::Command<crate::Data, crate::Error>> { vec![] }" > src/commands/mod.rs

# Build dependencies with persistent BuildKit caches
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --release

# Replace temporary source with the actual project
RUN rm -rf src
COPY src/ src/

# Build the actual application
RUN find src -type f -name '*.rs' -exec touch {} +
RUN cargo build --release

# --- Runtime Stage ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libopus0 \
    libssl3 \
    libfontconfig1 \
    libfreetype6 \
    fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/cyberpup-bot /usr/local/bin/cyberpup-bot

CMD ["cyberpup-bot"]