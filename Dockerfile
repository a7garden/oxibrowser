# Build stage
FROM rust:1-bookworm AS builder

WORKDIR /usr/src/oxibrowser

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY crates/oxibrowser/Cargo.toml crates/oxibrowser/Cargo.toml
COPY crates/oxibrowser-core/Cargo.toml crates/oxibrowser-core/Cargo.toml
COPY crates/oxibrowser-cdp/Cargo.toml crates/oxibrowser-cdp/Cargo.toml
COPY crates/oxibrowser-webapi/Cargo.toml crates/oxibrowser-webapi/Cargo.toml

# Create dummy source files to cache deps
RUN mkdir -p crates/oxibrowser/src && echo "fn main() {}" > crates/oxibrowser/src/main.rs \
    && mkdir -p crates/oxibrowser-core/src && touch crates/oxibrowser-core/src/lib.rs \
    && mkdir -p crates/oxibrowser-cdp/src && touch crates/oxibrowser-cdp/src/lib.rs \
    && mkdir -p crates/oxibrowser-webapi/src && touch crates/oxibrowser-webapi/src/lib.rs

RUN cargo build --release -p oxibrowser 2>/dev/null || true

# Copy real source and build
COPY . .
RUN touch crates/*/src/*.rs crates/*/src/**/*.rs \
    && cargo build --release -p oxibrowser

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/oxibrowser/target/release/oxibrowser /usr/local/bin/oxibrowser

# Default CDP port
EXPOSE 9222

ENTRYPOINT ["oxibrowser"]
CMD ["serve"]
