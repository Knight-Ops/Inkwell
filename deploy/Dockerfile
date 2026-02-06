# Build stage for both Server and Client
FROM rust:1.93-slim as builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    sqlite3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install WASM target and Trunk
RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk

# Copy workspace files
COPY . .

# Build the client first
WORKDIR /app/inkwell-client
RUN trunk build --release

# Build the server
WORKDIR /app
RUN cargo build --release -p inkwell-server --bin inkwell-server

# Final stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    openssl \
    ca-certificates \
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the server binary
COPY --from=builder /app/target/release/inkwell-server /app/inkwell-server

# Copy the compiled client assets
COPY --from=builder /app/inkwell-client/dist /app/dist

# Ensure the images directory exists (populated via ingest tool or volume mount)
RUN mkdir -p /app/card_images

# Environment setup
ENV DATABASE_URL=sqlite:/app/data/inkwell.db
EXPOSE 4000

CMD ["/app/inkwell-server"]
