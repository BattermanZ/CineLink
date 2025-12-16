FROM rust:1.92-alpine AS chef
WORKDIR /app
RUN apk add --no-cache musl-dev pkgconfig && \
    rustup target add x86_64-unknown-linux-musl && \
    cargo install cargo-chef

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
COPY . .
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
RUN cargo build --release --target x86_64-unknown-linux-musl --bin cinelink_server

FROM gcr.io/distroless/static-debian13
WORKDIR /app
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/cinelink_server /app/cinelink_server
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
USER nonroot
ENTRYPOINT ["/app/cinelink_server"]
