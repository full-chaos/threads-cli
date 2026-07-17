use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    #[serde(
        default,
        alias = "granted_scopes",
        skip_serializing_if = "Option::is_none"
    )]
    pub requested_scopes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub issued_at: DateTime<Utc>,
}

impl Token {
    pub fn new(
        access_token: impl Into<String>,
        expires_in: Option<i64>,
        requested_scopes: Option<Vec<String>>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            expires_in,
            requested_scopes,
            user_id: None,
            issued_at: Utc::now(),
        }
    }

    pub fn with_user_id(mut self, user_id: Option<String>) -> Self {
        self.user_id = user_id;
        self
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_in {
            Some(seconds) if seconds > 0 => {
                Utc::now()
                    .signed_duration_since(self.issued_at)
                    .num_seconds()
                    >= seconds
            }
            _ => false,
        }
    }
}

pub fn token_has_scope(token: &Token, scope: &str) -> bool {
    token
        .requested_scopes
        .as_ref()
        .is_some_and(|scopes| scopes.iter().any(|recorded| recorded == scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_token_lacks_new_scopes() {
        let token: Token =
            serde_json::from_str(r#"{"access_token":"t","issued_at":"2026-01-01T00:00:00Z"}"#)
                .unwrap();
        assert!(!token_has_scope(&token, "threads_delete"));
    }

    #[test]
    fn legacy_granted_scopes_deserialize_as_requested_scopes() {
        let token: Token = serde_json::from_str(
            r#"{"access_token":"t","granted_scopes":["threads_basic"],"issued_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(token_has_scope(&token, "threads_basic"));
    }
}
