# threads-provider-web

**Status: experimental, disabled by default.**

Per the PRD (`threads_api_cli_prd_correction.md`), this crate is a placeholder
for an optional read-only enrichment adapter against the private
`threads.net/api/graphql` endpoint. It is NOT part of v1.

To opt in for future work:

```bash
cargo build -p threads-provider-web --features enabled
```

The primary provider for all v1 functionality is
[`threads-provider-official`](../threads-provider-official/README.md), backed by
`https://graph.threads.net`.
