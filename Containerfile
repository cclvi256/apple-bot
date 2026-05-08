FROM docker.io/library/rust:latest AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN cargo fetch --locked
COPY src ./src
RUN cargo build --release --locked

FROM debian:stable-slim
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cider-bot /usr/local/bin/cider-bot
EXPOSE 8888
CMD ["cider-bot"]
