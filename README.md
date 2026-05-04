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

## Commands

Read-only ingest + query (always safe):

```
threads-cli init
threads-cli auth login | status | logout
threads-cli ingest me | thread <post_id> | engagement [--depth N]
threads-cli show <post_id> [--thread]
threads-cli search "<query>"
threads-cli export --format json|jsonl|csv
```

Destructive remote ops (dry-run by default; `--apply` actually performs the
delete via Meta's `DELETE /v1.0/{id}` endpoint):

```
threads-cli delete posts   [--before <date>] [--after <date>] [--apply] [--limit N]
threads-cli delete replies [--before <date>] [--after <date>] [--apply] [--limit N] [--yes-undocumented]
```

Filtering uses `posts.created_at` from the local store; `--before`/`--after`
accept either RFC 3339 (`2025-01-15T00:00:00Z`) or bare ISO date
(`2025-01-15`). The Threads API enforces a hard cap of 100 deletions per
24h; `delete` refuses cleanly when the cap is reached and reports when the
quota will reset. See [`docs/plans/delete.md`](docs/plans/delete.md) for the
full design.

Publishing (`threads_publish`), `archive` (Meta does not expose a remote
archive endpoint for root posts), multi-account, and the private
`threads.net/api/graphql` adapter are deferred past v1.

## License

Dual-licensed under MIT OR Apache-2.0.
