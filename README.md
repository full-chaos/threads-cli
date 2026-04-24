# threads-cli

A Rust CLI for ingesting, modeling, searching, and exporting
[Threads](https://www.threads.net/) content using the official
[Threads Graph API](https://developers.facebook.com/docs/threads) at
`https://graph.threads.net` as the primary provider.

> **Important:** despite the name, `graph.threads.net` is Meta's **REST-like
> Graph API**, not a GraphQL endpoint. `threads-cli` drives it from a versioned
> local TOML manifest and normalizes every response into a stable internal
> graph model before persisting to SQLite.

## Status

Phase 0 foundation scaffolding. See
[`docs/architecture.md`](docs/architecture.md) and
[`threads_api_cli_prd_correction.md`](threads_api_cli_prd_correction.md).

## Workspace layout

```
crates/
  threads-core/                 # Provider trait + internal graph model
  threads-manifest/             # TOML API manifest parser
  threads-provider-official/    # https://graph.threads.net client
  threads-provider-web/         # EXPERIMENTAL (disabled by default)
  threads-store/                # SQLite schema + FTS5 queries
  threads-ingest/               # Normalizer + pagination orchestrator
  threads-cli/                  # Binary (clap subcommands)
manifests/official_v1.toml      # Versioned API contract
```

## Quick build

```bash
cargo build --workspace
cargo test  --workspace
```

## Planned commands (v1, read-only)

```
threads-cli init
threads-cli auth login | status
threads-cli ingest me | thread <post_id>
threads-cli show <post_id> [--thread]
threads-cli search "<query>"
threads-cli export --format json|jsonl|csv
```

Publishing (`threads_publish`), multi-account, and the private
`threads.net/api/graphql` adapter are deferred past v1.

## License

Dual-licensed under MIT OR Apache-2.0.
