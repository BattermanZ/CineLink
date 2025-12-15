FROM rust:1.92-slim-bookworm AS chef
WORKDIR /app
RUN cargo install cargo-chef

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY tests tests
RUN cargo chef prepare --recipe recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
COPY . .
RUN cargo chef cook --release --recipe recipe.json
RUN cargo build --release --bin cinelink_server

FROM gcr.io/distroless/cc-debian13
WORKDIR /app
COPY --from=builder /app/target/release/cinelink_server /app/cinelink_server
USER nonroot
ENTRYPOINT ["/app/cinelink_server"]
