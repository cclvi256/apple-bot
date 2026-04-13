FROM docker.io/library/rust:1.90 AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN cargo fetch --locked
COPY src ./src
RUN cargo build --release --locked

FROM debian:13-slim
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/apple-bot /usr/local/bin/apple-bot
EXPOSE 8888
CMD ["apple-bot"]