# Build stage
FROM rust:1.92-slim-bookworm AS builder
WORKDIR /app

# Cache deps
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --bin cinelink_server

# Runtime stage (small Debian with certs)
FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cinelink_server /app/cinelink_server
USER nobody
ENTRYPOINT ["/app/cinelink_server"]
