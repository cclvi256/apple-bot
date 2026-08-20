# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 workspace with one package in `crates/cider-bot`. Runtime code lives in `crates/cider-bot/src`: `server.rs` handles Axum HTTP routes, `bot.rs` owns feature behavior, `protocol.rs` defines OneBot payloads, `napcat.rs` sends actions, and `store.rs` provides SQLx persistence. Keep custom errors in `error.rs` and configuration parsing in `config.rs`. SQLite and PostgreSQL migrations are separated under `migrations/{sqlite,postgres}`. Deployment assets are `Containerfile`, `compose.yaml`, and the future K3s template in `deploy/`. Architectural notes live in `docs/`; configuration templates live in `config/`.

## Build, Test, and Development Commands

- `cargo build --workspace` compiles all workspace members.
- `cargo run -p cider-bot` runs the bot using `CIDER_CONFIG_FILE` and `CIDER_SECRET_FILE` (copy the examples described in `README.md` first).
- `cargo fmt --check` verifies standard Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings` treats all lint warnings as failures.
- `cargo test --workspace` runs the ordinary unit and async integration-style tests.
- `cargo build --workspace --release` checks the production build.
- `podman compose config` validates the Compose definition; `podman compose up -d --build` starts the containerized development stack.

## Coding Style & Naming Conventions

Use `rustfmt` defaults (four-space indentation) and keep Clippy clean. Follow Rust conventions: `snake_case` for modules, functions, and tests; `UpperCamelCase` for types and traits; `SCREAMING_SNAKE_CASE` for constants. Prefer typed `thiserror` errors and workspace-managed dependencies. Keep feature manifests flat, schema-validated TOML; do not introduce nested manifest keys without an explicit format change.

## Testing Guidelines

Tests are colocated in `#[cfg(test)]` modules and use `#[test]` or `#[tokio::test]`. Name tests after observable behavior, for example `rejects_invalid_signature`. Add regression tests with behavior changes. Ordinary tests must remain offline: use temporary SQLite databases and mock HTTP servers, and do not launch NapCat or contact QQ. PostgreSQL coverage is opt-in:

```sh
TEST_POSTGRES_URL='postgres://...' cargo test -p cider-bot store::tests::postgres_feature_store_contract -- --ignored --exact
```

## Commit & Pull Request Guidelines

Follow the established imperative Conventional Commit style: `feat: ...`, `fix: ...`, `chore: ...`, or scoped forms such as `refactor(error): ...`. Keep commits focused. Pull requests should explain the behavior and configuration/schema impact, link relevant issues, list verification commands, and call out untested external systems. Include screenshots only for user-visible WebUI or deployment changes; never commit real tokens, QQ data, `.env`, or populated secret files.
