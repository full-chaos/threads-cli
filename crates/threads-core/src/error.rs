use std::time::Duration;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication error: {0}")]
    Auth(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("rate limited; retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    #[error("parse error: {0}")]
    Parse(String),

    #[error("manifest error: {0}")]
    Manifest(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(String),
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Parse(err.to_string())
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::Parse(format!("invalid url: {err}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Other(format!("io: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_forms_are_human_readable() {
        assert_eq!(Error::Auth("bad token".into()).to_string(), "authentication error: bad token");
        assert_eq!(
            Error::RateLimit { retry_after: Some(Duration::from_secs(30)) }.to_string(),
            "rate limited; retry after Some(30s)"
        );
        assert_eq!(Error::NotFound("post".into()).to_string(), "not found: post");
    }

    #[test]
    fn serde_json_error_maps_to_parse() {
        let err: Error = serde_json::from_str::<u32>("notanumber").unwrap_err().into();
        assert!(matches!(err, Error::Parse(_)));
    }

    #[test]
    fn url_error_maps_to_parse() {
        let err: Error = url::Url::parse("not a url").unwrap_err().into();
        assert!(matches!(err, Error::Parse(_)));
    }
}
