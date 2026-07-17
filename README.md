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

Audience snapshots, observed-engagement queries, and the official Follow
Intent command are implemented. Private follower enumeration and automated
follow remain unsupported. See
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

## Token persistence

`auth login` always atomically mirrors the access token and its metadata to a
private token file, then saves the same value to Keychain on a best-effort
basis. Reads are file-first; Keychain is consulted only when the private file
is absent. On Unix, the file is accepted only when it is an owner-only regular
file in an owner-controlled directory that is not group- or world-writable.

## Commands

Local ingest/query and user-mediated intent commands:

```
threads-cli init
threads-cli auth login | status | logout
threads-cli ingest me | thread <post_id> | engagement [--depth N]
threads-cli show <post_id> [--thread]
threads-cli search "<query>"
threads-cli export --format json|jsonl|csv
threads-cli follow <username> [--no-open]
threads-cli audience refresh
threads-cli audience show [--history N]
threads-cli audience engaged [--limit N] [--sort total|replies|mentions]
threads-cli audience purge --before <date> [--apply]
```

`follow` prints and optionally opens Meta's official Follow Intent. It does
not perform or confirm a follow. `audience refresh` fetches aggregate
`followers_count`, eligible country/city/age/gender demographics, and
official public mentions. `show`, `engaged`, and `purge` are local and use the
token-bound account. Engagement is based on observed direct replies and
mentions, not a follower list; engaged accounts must never be interpreted as
followers. Audience snapshots are private local data, excluded from post
export, and explicitly removable with `audience purge`.

Audience defaults: `show --history 10`, `engaged --limit 20 --sort total`.
`purge` is a dry run unless `--apply` is supplied. `refresh` requires the
recorded `threads_manage_insights` and `threads_manage_mentions` scopes.
The broad login requests exactly these six scopes:
`threads_basic`, `threads_read_replies`, `threads_delete`,
`threads_content_publish`, `threads_manage_insights`, and
`threads_manage_mentions`. Requested scopes are not proof that Meta granted
them; App Review and the user grant control access. The Mentions live gate is
EXTERNALLY UNVERIFIED.

Mentions uses Meta's cursor pagination. `audience refresh` selects 100 items
per page as this client's implementation choice; it is not an asserted Meta
maximum.

After the Insights snapshot is committed, only a Mentions permission denial is
downgraded to a warning. That warning names the required
`threads_manage_mentions` scope and recommends `threads-cli auth login` to
request it again. Other Mentions-phase errors (authentication, network, parse,
rate-limit, or store failures) fail `audience refresh`; the already committed
Insights snapshot remains available locally.

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

Publishing is supported. `archive` (Meta does not expose a remote archive
endpoint for root posts), multi-account, and the private
`threads.net/api/graphql` adapter remain outside the supported v1 surface.
The web adapter is disabled by default and read-only; it must not be used for
follower enumeration or automated follow actions.

## License

Dual-licensed under MIT OR Apache-2.0.
