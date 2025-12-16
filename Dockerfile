# Builder stage: compile a static musl binary so the runtime image can be distroless.
FROM rust:1.92-alpine AS builder
WORKDIR /app

# Install musl toolchain deps and CA cert bundle (used both at build time and copied into runtime).
RUN apk add --no-cache musl-dev ca-certificates && \
    rustup target add x86_64-unknown-linux-musl

# Copy only what we need to compile the server binary (keeps build context small via .dockerignore).
COPY Cargo.toml Cargo.lock ./
COPY src src

# Produce a release binary for the musl target.
RUN cargo build --release --target x86_64-unknown-linux-musl --bin cinelink_server

# Runtime stage: distroless static image (no shell/package manager). We copy only the binary + certs.
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
ENTRYPOINT ["/app/cinelink_server"]
