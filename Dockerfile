FROM rust:1.88 as builder
WORKDIR /app
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY proto ./proto
COPY build.rs ./
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
# Install OpenSSL runtime library and CA certificates
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/matchmaking /app/
CMD ["./matchmaking"]
