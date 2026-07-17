# Feasibility: followers and follow

- **Status:** Approved for the supported v1 surface.
- **Decision:** **Option A — aggregate audience data plus the official Follow
  Intent.** Approved 2026-07-16.
- **Scope:** `threads-cli` may show aggregate follower insights, retain local
  snapshot history, rank observed engagement, and hand a user a Follow Intent.

## Decision

The official Threads API does not expose a public follower/following edge or a
programmatic follow/unfollow mutation. It does expose `followers_count` and
`follower_demographics`, but not follower names. The shipped `follow` command
therefore uses Meta's **user-mediated** Web Intent:

```text
https://www.threads.com/intent/follow?username=<encoded-username>
```

The command always prints this URL and, unless `--no-open` is supplied, asks
the local browser to open it. Threads performs the interaction; this CLI has
no callback, no completion signal, and must not claim that a follow happened.
The user may sign in or cancel in Threads.

This is not a private endpoint, session-cookie flow, or API follow mutation.

## What audience refresh can read

`GET /{threads-user-id}/threads_insights` supports:

- `followers_count`: the current aggregate total. It does not support
  `since`/`until`.
- `follower_demographics`: aggregate buckets for `country`, `city`, `age`, or
  `gender`. A profile needs at least 100 followers; each request has exactly
  one `breakdown`; this metric also does not support `since`/`until`.

The CLI stores each successful refresh as an account-scoped local snapshot.
History and deltas are therefore **local observations over time**, not a
historical series supplied by Meta. A count decrease is valid. Refresh stores
the count and eligible demographic rows atomically. After that commit, only a
Mentions permission denial becomes a warning; authentication, network, parse,
rate-limit, and store failures fail the command without discarding the
snapshot. The permission warning names `threads_manage_mentions` and advises
running `threads-cli auth login` to request it again.

Mentions use `GET /{threads-user-id}/mentions`, with cursor pagination and a
limit of 100. The endpoint returns public media that tags the authenticated
profile. Private media is excluded. Without advanced access for
`threads_manage_mentions`, the documented behavior is tester-only results;
after approval, public posts from other users may be returned.

The official permissions material is currently inconsistent: the Mentions and
Insights endpoint pages require `threads_manage_mentions` and
`threads_manage_insights`, while the access-token scope table does not list
those two names in its displayed values. The CLI requests the broad six-scope
login and records requested scopes; it does not treat requested scopes as
granted scopes. A real authorization/API check for Mentions is **EXTERNALLY
UNVERIFIED** in this worktree.

The six requested scopes are:

```text
threads_basic
threads_read_replies
threads_delete
threads_content_publish
threads_manage_insights
threads_manage_mentions
```

The broad login is intentional: one consent flow supports the shipped read,
publish, delete, Insights, and Mentions paths. App Review and the user's actual
grant still control access; a requested permission is not proof of approval.

## Engagement boundary

`audience engaged` ranks only locally observed, official records: direct
replies to the account's posts and official Mentions records. It persists
authoritative `(user_id, username)` pairs when available and reconciles handles
without inventing identity. It does not infer follower status from replies,
mentions, quotes, demographics, or profile visibility. It does not rank
unknown quote authors. **Engaged does not mean follower.**

## CLI surface and privacy

```text
threads-cli follow <username> [--no-open]
threads-cli audience refresh
threads-cli audience show [--history N]       # N defaults to 10
threads-cli audience engaged [--limit N] [--sort total|replies|mentions]
                                             # defaults: 20, total
threads-cli audience purge --before <date> [--apply]
```

`refresh` is the remote command and requires the recorded Insights and
Mentions scopes before network setup. `show`, `engaged`, and `purge` are local
commands bound to the token's account. Purge is a dry run unless `--apply` is
given; its cutoff must not be in the future. Audience data is account-scoped
private local data, is not included in the existing post export, and can be
removed explicitly with `audience purge`.

## Rejected approaches

**Option C is rejected.** This project will not ship follower scraping,
private `threads.net/api/graphql` or friendships endpoints, session-cookie
authentication, automated follow/unfollow, or a writable web provider. The web
adapter remains disabled by default and read-only. These approaches are
unsupported, brittle, and contrary to the project's privacy and platform
policy boundary; no fake manifest stubs (`documented = false`) are used to
imply an API contract that does not exist.

## References

- Meta, Threads API overview: <https://developers.facebook.com/docs/threads>
- Meta, Insights: <https://developers.facebook.com/docs/threads/insights>
- Meta, Mentions: <https://developers.facebook.com/docs/threads/threads-mentions>
- Meta, Web Intents: <https://developers.facebook.com/docs/threads/threads-web-intents>
- Meta, Get Started and permissions: <https://developers.facebook.com/docs/threads/get-started/>
- Meta, access tokens and permissions: <https://developers.facebook.com/docs/threads/get-started/get-access-tokens-and-permissions/>
- Repository architecture: [`architecture.md`](architecture.md)
