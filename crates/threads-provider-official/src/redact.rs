//! Defensive redaction of secrets in strings that flow to logs / `Error`
//! variants / audit rows.
//!
//! ## Why this exists
//!
//! Every request `HttpClient` makes appends `?access_token=<bearer>` to the
//! query string, and every OAuth call POSTs `client_secret=` / `code=` /
//! `access_token=` form fields. When an HTTP call fails, Meta's response body
//! often echoes the request URL or includes OAuth context that contains those
//! secrets verbatim. If we forward that body into a `format!("…: {body}")`
//! that becomes part of an `Error::Auth(...)` or `Error::Other(...)`, the
//! secret eventually surfaces in:
//!
//! - `tracing::*` output (operator log files)
//! - the CLI's `eprintln!("failed to delete {}: {err}", id)` (terminal/CI logs)
//! - the `deletions` audit table (`error` column)
//!
//! That's the textbook **CWE-532: cleartext logging of sensitive information**
//! pattern. CodeQL's `rust/cleartext-logging-of-sensitive-info` query catches
//! it; humans miss it because the data flow crosses three modules.
//!
//! ## What this module does
//!
//! [`redact`] takes a string and returns a copy with `access_token`,
//! `client_secret`, `code`, and `refresh_token` values replaced by a literal
//! `[REDACTED]` placeholder. It handles three shapes seen in Meta's responses:
//!
//! 1. URL query: `?access_token=THE_BEARER` or `&client_secret=...`
//! 2. JSON: `"access_token":"THE_BEARER"` (with optional whitespace)
//! 3. URL-encoded form bodies: `access_token=THE_BEARER&...`
//!
//! ## What this module does NOT do
//!
//! - Detect arbitrary new secret-shaped strings. The set of redacted keys is
//!   explicit and conservative; if Meta adds a new one, we add it here.
//! - Promise zero leakage. `redact` is a defense-in-depth layer; the
//!   primary mitigation remains "don't put secrets in error bodies in the
//!   first place" (i.e., make sure callers go through this helper).

/// Substring inserted in place of any redacted secret.
pub const PLACEHOLDER: &str = "[REDACTED]";

/// Keys whose values must never reach a log / Error / audit row.
///
/// Order matters only for readability; matching is exact-key + case-sensitive
/// because Meta's API and OAuth specs use these exact lowercase identifiers.
const SENSITIVE_KEYS: &[&str] = &["access_token", "client_secret", "refresh_token", "code"];

/// Return a copy of `s` with sensitive query / JSON / form values replaced
/// by [`PLACEHOLDER`].
///
/// Idempotent: redacting an already-redacted string is a no-op (
/// `[REDACTED]` is its own value).
pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for key in SENSITIVE_KEYS {
        out = redact_query_pair(&out, key);
        out = redact_json_pair(&out, key);
    }
    out
}

/// Replace `key=VALUE` (URL query / form body) with `key=[REDACTED]`.
///
/// `VALUE` runs from the `=` to the next `&`, end of string, whitespace, or
/// closing quote / brace. We deliberately stop at common delimiters rather
/// than over-redacting downstream context.
fn redact_query_pair(input: &str, key: &str) -> String {
    let needle = format!("{key}=");
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find(&needle) {
        out.push_str(&rest[..idx]);
        out.push_str(&needle);
        out.push_str(PLACEHOLDER);
        let after = &rest[idx + needle.len()..];
        // Skip the original value: stop at the first delimiter we recognize.
        let end = after
            .find(['&', ' ', '\n', '\r', '\t', '"', '}', ')'].as_slice())
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Replace JSON `"key": "VALUE"` (with arbitrary whitespace around `:`) with
/// `"key":"[REDACTED]"`. Tolerates both `"key":"v"` and `"key" : "v"` forms.
fn redact_json_pair(input: &str, key: &str) -> String {
    let needle = format!("\"{key}\"");
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find(&needle) {
        out.push_str(&rest[..idx]);
        out.push_str(&needle);
        let after = &rest[idx + needle.len()..];
        // Look for `:` then `"VALUE"` allowing whitespace.
        let trimmed = after.trim_start();
        if !trimmed.starts_with(':') {
            // `key` isn't a JSON key here (e.g. it appeared inside another
            // value). Skip without redacting.
            out.push_str(after);
            return out;
        }
        let after_colon = trimmed[1..].trim_start();
        if !after_colon.starts_with('"') {
            out.push_str(after);
            return out;
        }
        // Find the closing quote, accounting for the obvious case (no escaped
        // quotes — Meta's bearer tokens are URL-safe base64 or similar).
        let value_body = &after_colon[1..];
        let end = value_body.find('"').unwrap_or(value_body.len());
        out.push_str(":\"");
        out.push_str(PLACEHOLDER);
        out.push('"');
        rest = &value_body[end + 1.min(value_body.len() - end)..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_query_access_token_redacted() {
        let url = "https://graph.threads.net/v1.0/me?fields=id&access_token=ABC.DEF.123";
        let redacted = redact(url);
        assert!(redacted.contains("access_token=[REDACTED]"));
        assert!(!redacted.contains("ABC.DEF.123"));
        // Other params survive.
        assert!(redacted.contains("fields=id"));
    }

    #[test]
    fn url_query_token_in_middle() {
        let url = "?access_token=SECRET123&fields=id";
        let redacted = redact(url);
        assert_eq!(redacted, "?access_token=[REDACTED]&fields=id");
    }

    #[test]
    fn json_token_response_redacted() {
        let body = r#"{"access_token":"EAABwzLix...","token_type":"bearer","expires_in":5184000}"#;
        let redacted = redact(body);
        assert!(redacted.contains("\"access_token\":\"[REDACTED]\""));
        assert!(!redacted.contains("EAABwzLix"));
        // Non-secret fields survive.
        assert!(redacted.contains("\"token_type\":\"bearer\""));
        assert!(redacted.contains("\"expires_in\":5184000"));
    }

    #[test]
    fn json_token_with_whitespace_around_colon() {
        let body = r#"{"access_token" : "WHITESPACE_TOKEN"}"#;
        let redacted = redact(body);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("WHITESPACE_TOKEN"));
    }

    #[test]
    fn oauth_form_body_redacted() {
        let body =
            "client_id=APP&client_secret=DEADBEEF&code=AQX1234&grant_type=authorization_code";
        let redacted = redact(body);
        assert!(redacted.contains("client_secret=[REDACTED]"));
        assert!(redacted.contains("code=[REDACTED]"));
        // client_id is NOT considered sensitive (it's public).
        assert!(redacted.contains("client_id=APP"));
        // grant_type unaffected.
        assert!(redacted.contains("grant_type=authorization_code"));
    }

    #[test]
    fn refresh_token_redacted() {
        let body = r#"{"refresh_token":"R3FR3SH"}"#;
        let redacted = redact(body);
        assert!(!redacted.contains("R3FR3SH"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn empty_input_unchanged() {
        assert_eq!(redact(""), "");
    }

    #[test]
    fn no_secrets_unchanged() {
        let s = "{\"error\":{\"message\":\"insufficient permissions\",\"code\":190,\"type\":\"OAuthException\"}}";
        // `code` here is a JSON KEY for the numeric Meta error code — it has
        // no string value to leak, so the JSON-pair redactor leaves it alone
        // (it only matches `"code":"..."` with a string value). Numeric
        // values like `"code":190` don't get touched.
        let redacted = redact(s);
        assert_eq!(redacted, s);
    }

    #[test]
    fn redaction_is_idempotent() {
        let url = "?access_token=[REDACTED]&x=y";
        let once = redact(url);
        let twice = redact(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn meta_error_body_with_echoed_url() {
        // Real-world shape: Meta's OAuthException sometimes echoes the
        // request line including the bearer token.
        let body = r#"{"error":{"message":"Invalid OAuth access token","type":"OAuthException","code":190,"fbtrace_id":"AY"},"request":"GET /v1.0/me?access_token=BAAOOPS"}"#;
        let redacted = redact(body);
        assert!(!redacted.contains("BAAOOPS"));
        assert!(redacted.contains("[REDACTED]"));
        // Diagnostic context survives.
        assert!(redacted.contains("OAuthException"));
        assert!(redacted.contains("fbtrace_id"));
    }
}
