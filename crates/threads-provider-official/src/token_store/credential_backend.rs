pub(super) type CredentialResult<T> = std::result::Result<T, String>;

pub(super) trait CredentialBackend: Send + Sync {
    fn save(&self, secret: &str) -> CredentialResult<()>;
    fn load(&self) -> CredentialResult<Option<String>>;
    fn clear(&self) -> CredentialResult<()>;
}

pub(super) struct KeyringBackend;

#[cfg(feature = "test-support")]
pub(super) struct FileOnlyBackend;

impl CredentialBackend for KeyringBackend {
    fn save(&self, secret: &str) -> CredentialResult<()> {
        keyring_entry()?
            .set_password(secret)
            .map_err(|error| error.to_string())
    }

    fn load(&self) -> CredentialResult<Option<String>> {
        keyring_entry()?
            .get_password()
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn clear(&self) -> CredentialResult<()> {
        clear_credential_result(keyring_entry()?.delete_credential())
    }
}

#[cfg(feature = "test-support")]
impl CredentialBackend for FileOnlyBackend {
    fn save(&self, _: &str) -> CredentialResult<()> {
        Ok(())
    }

    fn load(&self) -> CredentialResult<Option<String>> {
        Ok(None)
    }

    fn clear(&self) -> CredentialResult<()> {
        Ok(())
    }
}

fn keyring_entry() -> CredentialResult<keyring::Entry> {
    keyring::Entry::new("threads-cli", "default").map_err(|error| error.to_string())
}

fn clear_credential_result(result: keyring::Result<()>) -> CredentialResult<()> {
    match result {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::clear_credential_result;

    #[test]
    fn clear_treats_missing_keyring_entry_as_success() {
        assert!(clear_credential_result(Err(keyring::Error::NoEntry)).is_ok());
    }
}
