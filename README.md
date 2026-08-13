# cider-bot

`cider-bot` is a Rust OneBot 11 bot designed for NapCat. The bot core supports feature modules; dice statistics is the first and currently only module.

## Dice commands

- `.enable dice` — enable the feature (bot owner or group owner/admin).
- `.disable dice` — disable it and discard an active session (same permissions).
- `.dice` — start an in-memory session (any member). During an active session, its behavior is controlled by `session_mode`.
- `.ecid` — end the session and publish scores 6 through 1 (any member).

Only the first dice message from each QQ is counted. Active sessions disappear when the bot restarts. Group feature records are stored with SQLx in SQLite by default or PostgreSQL when the configured URL uses `postgres:`/`postgresql:`. Each record retains its enabled state and a format-tagged manifest when disabled. The manifest holds explicitly defined, non-default feature data. TOML is the only supported manifest format in this version.

The dice manifest supports `session_mode = "strict" | "common"`. The default is `common`, so the key may be omitted: `.dice` during an active session publishes its statistics and immediately starts a new session. `strict` preserves the earlier behavior and replies that a session is already active.

Commands may be preceded or followed by non-text segments, so `@bot .dice` works. Non-text segments between command text are preserved as typed arguments; the current commands reject them because their usage has only text arguments.

## Configuration

Copy the examples before starting locally:

```sh
cp config/config.example.toml config/config.toml
cp config/secret.example.toml config/secret.toml
cp config/secret.postgres.example.toml config/secret.postgres.toml
cp .env.example .env
```

Edit the owner QQ IDs and both tokens. NapCat uses the webhook token to sign event bodies and the API token to authenticate action calls. The only supported environment overrides are `CIDER_CONFIG_FILE` and `CIDER_SECRET_FILE`; regular settings belong in TOML. `RUST_LOG` controls logging.

## Podman Compose development

The default stack contains only containerized NapCat and the SQLite-backed bot:

```sh
podman compose up -d --build
```

NapCat WebUI is available only from the local machine at <http://127.0.0.1:6099/webui>. Its QQ data and configuration use named volumes. No host installation of NapCat is needed.

To use containerized PostgreSQL, set matching `POSTGRES_PASSWORD` and DSN password, then run:

```sh
CIDER_SECRET_FILE=/etc/cider-bot/secret.postgres.toml \
  podman compose --profile postgres up -d --build
```

After logging into QQ through NapCat WebUI, configure OneBot 11 with array messages and `reportSelfMessage: false`:

1. Add an HTTP server listening on `0.0.0.0:3000`, using the `api_token` from `secret.toml`.
2. Add an HTTP client targeting `http://cider-bot:8080/onebot/v11/events`, using the `webhook_token` from `secret.toml`.

The HTTP client signs webhook bodies using `X-Signature`; the bot verifies it before parsing JSON. Replies use OneBot HTTP quick operations. See the [OneBot HTTP POST specification](https://github.com/botuniverse/onebot-11/blob/master/communication/http-post.md), [NapCat network configuration](https://napneko.github.io/config/basic), and [NapCat HTTP API](https://napcat.apifox.cn/).

## Kubernetes deployment artifact

[`deploy/k3s.yaml`](deploy/k3s.yaml) is a future K3s template and is not applied automatically. Replace every `REPLACE_...` value, the example hostname, Gateway reference, and image references before use. The existing Gateway listener must allow routes from the `cider-bot` namespace. The template deploys NapCat and the bot separately, expects an external PostgreSQL database, keeps OneBot endpoints cluster-internal, and exposes only NapCat WebUI through an `HTTPRoute`.

## Development checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
podman compose config
```

PostgreSQL store compatibility can be exercised when a test instance is available:

```sh
TEST_POSTGRES_URL='postgres://...' \
  cargo test -p cider-bot store::tests::postgres_feature_store_contract -- --ignored --exact
```

Ordinary tests create temporary SQLite databases and never start NapCat or contact QQ. The ignored mock-NapCat test only starts an ephemeral local HTTP server.
