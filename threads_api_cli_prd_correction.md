# Threads API CLI: PRD Correction

## Key Correction

Yes, the official endpoint is:

```text
https://graph.threads.net
```

Not:

```text
graph.threads.com/net
```

However, the important distinction is this:

> `graph.threads.net` is Meta’s official Threads Graph API endpoint, not a GraphQL schema endpoint.

It does **not** provide a GraphQL introspection schema via `__schema`. It follows Meta’s Graph API model: versioned resource paths, edges, fields, permissions, and access tokens.

---

## What `graph.threads.net` Provides

The official Threads API uses URLs like:

```bash
https://graph.threads.net/v1.0/me?fields=id,name
https://graph.threads.net/v1.0/me/threads
https://graph.threads.net/v1.0/me/threads_publish
```

This is REST-like Meta Graph API behavior, using:

- Versioned paths
- Object IDs
- Edges
- Field projection via `fields=`
- OAuth permissions
- Access tokens

It is not GraphQL, despite the word “Graph” in the API name.

---

## Why This Matters

The original idea was:

> Use their GraphQL spec and dynamically build modeling/pipelines.

That should be corrected to:

> Use the official Threads Graph API as the primary provider. Maintain a versioned local API manifest describing supported objects, fields, edges, permissions, and endpoint behavior. Generate typed Rust request/response models from that manifest where practical. Normalize all provider responses into a stable internal graph model.

This is a stronger architecture because it does not depend on unstable private web GraphQL internals.

---

## Corrected Architecture

```text
threads-cli
  providers/
    official_graph_api/     # graph.threads.net, primary
    web_graphql_private/    # threads.net/api/graphql, optional/experimental
  schema/
    official_api_manifest.toml
    field_sets.toml
  store/
    sqlite graph model
  ingest/
    crawlers and normalizers
```

---

## Provider Strategy

### Primary Provider: Official Threads Graph API

Use `graph.threads.net` for all supported operations.

Responsibilities:

- OAuth authentication
- Access token management
- Official resource fetching
- Official publishing/reply management where supported
- Permission-aware API access
- Rate limiting and retries
- Typed response handling

### Optional Provider: Private Threads Web GraphQL

Use only as an experimental, replaceable enrichment adapter.

Responsibilities:

- Read-only enrichment where official API is insufficient
- Operation registry based on known frontend operation/doc IDs
- Fixture-based validation
- Clear failure boundaries

This provider should be disabled by default.

---

## API Manifest Approach

Because `graph.threads.net` does not expose a GraphQL schema, the CLI should use a versioned local manifest.

Example:

```toml
[api]
base_url = "https://graph.threads.net"
version = "v1.0"

[[objects]]
name = "me"
path = "/v1.0/me"
method = "GET"
fields = [
  "id",
  "name",
  "username",
  "threads_profile_picture_url",
  "threads_biography"
]

[[edges]]
name = "threads"
path = "/v1.0/me/threads"
method = "POST"
permission = "threads_content_publish"

[[edges]]
name = "threads_publish"
path = "/v1.0/me/threads_publish"
method = "POST"
permission = "threads_content_publish"
```

---

## Modeling Rule

Do not generate the persistent database schema directly from provider responses.

Use this pattern:

```text
Official API response
  -> typed provider response
  -> normalizer
  -> stable internal model
  -> SQLite graph store
```

Avoid this pattern:

```text
Official API response
  -> dynamically generated database schema
```

Provider payloads change. Your internal model should not.

---

## Revised Product Foundation

The CLI should be built around a stable internal graph model:

- Users
- Posts
- Replies
- Parent-child reply edges
- Root-thread edges
- Mentions
- URLs
- Media
- Fetch provenance
- Raw provider payloads

SQLite remains the correct v1 storage choice.

Use:

- SQLite tables for normalized graph data
- `fts5` for text search
- recursive CTEs for branch traversal
- raw JSON columns for provider-specific payload retention

---

## Corrected Implementation Direction

### Instead of this

```text
GraphQL schema introspection -> generated models -> dynamic pipelines
```

### Do this

```text
Versioned API manifest -> typed official client -> normalized graph model -> SQLite store -> search/export/crawl pipelines
```

---

## Immediate Impact on the PRD

### Update the product description

The CLI is a Rust tool for ingesting, modeling, searching, and exporting Threads content using the official Threads Graph API as the primary data provider, with an optional experimental private GraphQL adapter for unsupported read-only enrichment.

### Update the provider model

```text
ProviderPriority:
  1. official_graph_api
  2. cache
  3. experimental_web_graphql
```

The private GraphQL adapter should not be second by default. It should require explicit opt-in.

### Update the schema generation plan

Generate Rust request/response structs from a local manifest and fixtures, not from live GraphQL introspection.

---

## Recommended Codex/Claude Instruction

```text
Revise the Threads CLI plan to treat https://graph.threads.net as the primary official Threads Graph API provider, not as a GraphQL introspection endpoint.

Implement a versioned local API manifest describing supported endpoints, fields, edges, required permissions, and request/response fixtures.

Do not generate the database schema from provider responses. Normalize all responses into a stable internal graph model consisting of users, posts, edges, media, URLs, crawl runs, and raw JSON payloads.

Keep any private threads.net/api/graphql adapter experimental, disabled by default, read-only, and isolated behind the same provider trait.
```

---

## Net Decision

Build around:

```text
https://graph.threads.net
```

But treat it as:

```text
Meta Graph API
```

Not:

```text
GraphQL
```

This improves the architecture. It makes the tool more durable, testable, and suitable for a Rust CLI that Codex or Claude can implement without chasing unstable private web internals.

