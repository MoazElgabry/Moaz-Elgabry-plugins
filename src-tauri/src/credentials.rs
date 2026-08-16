use anyhow::{anyhow, Context, Result};
use keyring::{Entry, Error as KeyringError};

const CREDENTIAL_SERVICE: &str = "com.moazelgabry.pluginmanager";
const CREDENTIAL_ACCOUNT: &str = "development-invitation";

pub fn validate_invitation_token(token: &str) -> Result<&str> {
    let token = token.trim();
    let payload = token
        .strip_prefix("mer_")
        .ok_or_else(|| anyhow!("Invitation keys must begin with mer_."))?;
    if payload.len() != 43
        || !payload
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(anyhow!("Invitation key format is invalid."));
    }
    Ok(token)
}

pub fn invitation_token() -> Result<Option<String>> {
    let entry = credential_entry()?;
    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(error)
            .context("Failed to read the development invitation from the OS credential store"),
    }
}

pub fn store_invitation_token(token: &str) -> Result<()> {
    let token = validate_invitation_token(token)?;
    credential_entry()?
        .set_password(token)
        .context("Failed to store the development invitation in the OS credential store")
}

pub fn forget_invitation_token() -> Result<()> {
    let entry = credential_entry()?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(error)
            .context("Failed to remove the development invitation from the OS credential store"),
    }
}

fn credential_entry() -> Result<Entry> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
        .context("Failed to open the OS credential store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitation_format_accepts_the_wordpress_token_shape() {
        let token = format!("mer_{}", "A".repeat(43));
        assert_eq!(validate_invitation_token(&token).unwrap(), token);
    }

    #[test]
    fn invitation_format_rejects_values_that_could_be_logged_as_normal_text() {
        assert!(validate_invitation_token("not-a-token").is_err());
        assert!(validate_invitation_token(&format!("mer_{}", "A".repeat(42))).is_err());
        assert!(validate_invitation_token(&format!("mer_{}!", "A".repeat(42))).is_err());
    }
}
