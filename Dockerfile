# Build stage
FROM rust:1-bookworm AS builder

# System packages required by the Blitz / vello_cpu / fontconfig stack.
RUN apt-get update && apt-get install -y --no-install-recommends \
    libfontconfig1-dev \
    libfreetype-dev \
    libexpat1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/oxibrowser

# Cache dependencies — copy only manifests first
COPY Cargo.toml Cargo.lock ./
COPY crates/oxibrowser/Cargo.toml crates/oxibrowser/Cargo.toml
COPY crates/oxibrowser-core/Cargo.toml crates/oxibrowser-core/Cargo.toml
COPY crates/oxibrowser-cdp/Cargo.toml crates/oxibrowser-cdp/Cargo.toml
COPY crates/oxibrowser-render/Cargo.toml crates/oxibrowser-render/Cargo.toml

# Create dummy source files to cache deps
RUN mkdir -p crates/oxibrowser/src && echo "fn main() {}" > crates/oxibrowser/src/main.rs \
    && mkdir -p crates/oxibrowser-core/src && touch crates/oxibrowser-core/src/lib.rs \
    && mkdir -p crates/oxibrowser-cdp/src && touch crates/oxibrowser-cdp/src/lib.rs \
    && mkdir -p crates/oxibrowser-render/src && touch crates/oxibrowser-render/src/lib.rs

# --features browser is required: [[bin]] required-features gates the binary
RUN cargo build --release --features browser -p oxibrowser 2>/dev/null || true

# Copy real source and build
COPY . .
RUN touch crates/*/src/*.rs crates/*/src/**/*.rs \
    && cargo build --release --features browser -p oxibrowser

# Runtime stage
FROM debian:bookworm-slim

# Runtime shared libraries linked by the Blitz / fontconfig stack
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libfontconfig1 \
    libfreetype6 \
    libexpat1 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/oxibrowser/target/release/oxibrowser /usr/local/bin/oxibrowser

# Default CDP port
EXPOSE 9222

ENTRYPOINT ["oxibrowser"]
CMD ["serve"]
