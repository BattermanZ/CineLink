# Stage 1: Base image with cargo-chef installed
FROM rust:1.82.0-slim-bookworm AS chef
WORKDIR /app

# Install necessary build tools and dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-chef
RUN cargo install cargo-chef

# Stage 2: Planning stage to prepare the build recipe
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Building dependencies based on the recipe
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build the actual application
COPY . .
RUN cargo build --release

# Stage 4: Create a minimal runtime image
FROM debian:bookworm-slim
WORKDIR /app

# Install necessary runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the built binary from the builder stage
COPY --from=builder /app/target/release/CineLink .

# Create directory for logs
RUN mkdir -p /app/logs

# Expose the port the app runs on
EXPOSE ${PORT:-3146}

# Set the command to run the CineLink application
CMD ["./CineLink"]

