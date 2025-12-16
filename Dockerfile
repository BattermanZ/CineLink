# Step 1: Chef stage (tooling)
FROM rust:1.92-alpine AS chef
WORKDIR /app
# Install musl toolchain deps and CA cert bundle (also copied into runtime image).
RUN apk add --no-cache musl-dev ca-certificates && \
    rustup target add x86_64-unknown-linux-musl
# Cargo-chef speeds up rebuilds by caching dependency compilation in a separate layer.
RUN cargo install cargo-chef

# Step 2: Planner stage (dependency recipe)
# - Generates recipe.json used to cache Rust dependencies across rebuilds
FROM chef AS planner
# Copy only what is needed for dependency analysis.
COPY Cargo.toml Cargo.lock ./
COPY src src
# Generate the cargo-chef recipe.
RUN cargo chef prepare --recipe-path recipe.json

# Step 3: Builder stage (compile + link)
# - Builds dependencies using the recipe (cacheable)
# - Builds the final release binary for musl
FROM chef AS builder
# Reuse the dependency recipe from the planner stage.
COPY --from=planner /app/recipe.json /app/recipe.json
# Build dependencies based on the recipe (cached across rebuilds).
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
# Copy the full app source (filtered by .dockerignore).
COPY . .
# Produce the release binary for the musl target.
RUN cargo build --release --target x86_64-unknown-linux-musl --bin cinelink_server

# Step 4: Runtime stage (distroless)
# - Copies the static binary + CA bundle into a minimal runtime image
FROM gcr.io/distroless/static-debian13
WORKDIR /app
# App binary.
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/cinelink_server /app/cinelink_server
# CA certificates for outbound HTTPS (Notion + TMDB).
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
# Tell TLS libraries where to find the CA bundle in this minimal image.
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
# Webhook listener port.
EXPOSE 3146
# Drop privileges.
USER nonroot
# Start the server.
ENTRYPOINT ["/app/cinelink_server"]
