# Builder stage
# Use a specific Debian version (bookworm) to match the runtime image and avoid GLIBC incompatibilities
FROM rust:1.92-bookworm as builder

WORKDIR /usr/src/app

# Copy manifests and config to cache dependencies properly
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo .cargo

# Create a dummy main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies
RUN cargo build --release

# Remove the dummy main.rs
RUN rm -rf src

# Copy the actual source code
COPY . .

# Update the modification time of main.rs to ensure a rebuild
RUN touch src/main.rs

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install necessary runtime dependencies (SSL certificates for HTTPS)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from the builder stage
COPY --from=builder /usr/src/app/target/release/open-webui-spider-rs /usr/local/bin/open-webui-spider-rs

# Expose the application port
EXPOSE 3000

# Set the entrypoint
CMD ["open-webui-spider-rs"]
