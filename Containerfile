FROM docker.io/library/rust:1.90 AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src target
COPY src ./src
RUN cargo build --release

FROM debian:trixie-slim
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/apple-bot /usr/local/bin/apple-bot
EXPOSE 8888
CMD ["apple-bot"]