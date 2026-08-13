FROM docker.io/library/rust:trixie AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p cider-bot

FROM docker.io/library/debian:trixie-slim
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin cider \
    && install -d -o cider -g cider /data /etc/cider-bot
COPY --from=builder /build/target/release/cider-bot /usr/local/bin/cider-bot
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/cider-bot"]
