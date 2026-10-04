use anyhow::{anyhow, Context, Result};
use keyring::{Entry, Error as KeyringError};
use serde_json;

const CREDENTIAL_SERVICE: &str = "com.moazelgabry.pluginmanager";
const CREDENTIAL_ACCOUNT: &str = "development-invitation";
const RECEIPT_ACCOUNT: &str = "invitation-request-receipt";
const HYOGEN_LICENSE_KEY_ACCOUNT: &str = "hyogen-license-key";
const LICENSE_INDEX_ACCOUNT: &str = "license-key-index";
const LICENSE_KEY_PREFIX: &str = "license-key:";

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

pub fn validate_request_receipt(receipt: &str) -> Result<&str> {
    let receipt = receipt.trim();
    if !receipt.starts_with("mrr_") || receipt.len() < 20 || receipt.len() > 80 {
        return Err(anyhow!("Invitation request receipt format is invalid."));
    }
    if !receipt
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(anyhow!("Invitation request receipt format is invalid."));
    }
    Ok(receipt)
}

pub fn invitation_request_receipt() -> Result<Option<String>> {
    match receipt_entry()?.get_password() {
        Ok(receipt) => Ok(Some(validate_request_receipt(&receipt)?.to_owned())),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Failed to read the invitation request receipt"),
    }
}

pub fn store_invitation_request_receipt(receipt: &str) -> Result<()> {
    let receipt = validate_request_receipt(receipt)?;
    receipt_entry()?
        .set_password(receipt)
        .context("Failed to store the invitation request receipt")
}

pub fn forget_invitation_request_receipt() -> Result<()> {
    match receipt_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(error).context("Failed to remove the invitation request receipt"),
    }
}

pub fn validate_license_key(key: &str) -> Result<&str> {
    let key = key.trim();
    if key.len() < 8 || key.len() > 256 {
        return Err(anyhow!("License key format is invalid."));
    }
    if !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(anyhow!("License key format is invalid."));
    }
    Ok(key)
}

pub fn validate_hyogen_license_key(key: &str) -> Result<&str> {
    validate_license_key(key)
}

pub fn hyogen_license_key() -> Result<Option<String>> {
    match license_key_entry()?.get_password() {
        Ok(key) => Ok(Some(validate_license_key(&key)?.to_owned())),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Failed to read the Hyogen license key"),
    }
}

pub fn store_hyogen_license_key(key: &str) -> Result<()> {
    let key = validate_license_key(key)?;
    license_key_entry()?
        .set_password(key)
        .context("Failed to store the Hyogen license key in the OS credential store")
}

pub fn forget_hyogen_license_key() -> Result<()> {
    match license_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(error).context("Failed to remove the Hyogen license key"),
    }
}

pub fn validate_license_id(license_id: &str) -> Result<&str> {
    let license_id = license_id.trim();
    if license_id.is_empty() || license_id.len() > 160 {
        return Err(anyhow!("License identifier format is invalid."));
    }
    if !license_id.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
    }) {
        return Err(anyhow!("License identifier format is invalid."));
    }
    Ok(license_id)
}

pub fn license_key(license_id: &str) -> Result<Option<String>> {
    let license_id = validate_license_id(license_id)?;
    match license_entry(license_id)?.get_password() {
        Ok(key) => Ok(Some(validate_hyogen_license_key(&key)?.to_owned())),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Failed to read a license key from the OS credential store"),
    }
}

pub fn store_license_key(license_id: &str, key: &str) -> Result<()> {
    let license_id = validate_license_id(license_id)?;
    let key = validate_hyogen_license_key(key)?;
    license_entry(license_id)?
        .set_password(key)
        .context("Failed to store the license key in the OS credential store")?;
    let mut ids = license_ids()?;
    if !ids.iter().any(|value| value == license_id) {
        ids.push(license_id.to_owned());
        ids.sort();
        license_index_entry()?
            .set_password(&serde_json::to_string(&ids)?)
            .context("Failed to update the license key index")?;
    }
    Ok(())
}

pub fn forget_license_key(license_id: &str) -> Result<()> {
    let license_id = validate_license_id(license_id)?;
    match license_entry(license_id)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(error) => {
            return Err(error).context("Failed to remove the license key from the OS credential store")
        }
    }
    let ids = license_ids()?
        .into_iter()
        .filter(|value| value != license_id)
        .collect::<Vec<_>>();
    license_index_entry()?
        .set_password(&serde_json::to_string(&ids)?)
        .context("Failed to update the license key index")?;
    Ok(())
}

pub fn license_ids() -> Result<Vec<String>> {
    let raw = match license_index_entry()?.get_password() {
        Ok(raw) => raw,
        Err(KeyringError::NoEntry) => return Ok(Vec::new()),
        Err(error) => return Err(error).context("Failed to read the license key index"),
    };
    let ids = serde_json::from_str::<Vec<String>>(&raw)
        .context("The stored license key index is invalid JSON")?;
    let mut valid = ids
        .into_iter()
        .filter_map(|id| validate_license_id(&id).ok().map(str::to_owned))
        .collect::<Vec<_>>();
    valid.sort();
    valid.dedup();
    Ok(valid)
}

fn credential_entry() -> Result<Entry> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
        .context("Failed to open the OS credential store")
}

fn receipt_entry() -> Result<Entry> {
    Entry::new(CREDENTIAL_SERVICE, RECEIPT_ACCOUNT)
        .context("Failed to open the invitation request receipt store")
}

fn license_key_entry() -> Result<Entry> {
    Entry::new(CREDENTIAL_SERVICE, HYOGEN_LICENSE_KEY_ACCOUNT)
        .context("Failed to open the Hyogen license key store")
}

fn license_entry(license_id: &str) -> Result<Entry> {
    let account = format!("{LICENSE_KEY_PREFIX}{license_id}");
    Entry::new(CREDENTIAL_SERVICE, &account)
        .context("Failed to open the license key store")
}

fn license_index_entry() -> Result<Entry> {
    Entry::new(CREDENTIAL_SERVICE, LICENSE_INDEX_ACCOUNT)
        .context("Failed to open the license key index")
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

    #[test]
    fn request_receipt_format_is_strict() {
        assert!(validate_request_receipt("mrr_abcdefghijklmnop").is_ok());
        assert!(validate_request_receipt("receipt").is_err());
        assert!(validate_request_receipt("mrr_bad value").is_err());
    }

    #[test]
    fn hyogen_license_key_format_is_bounded_and_secret_safe() {
        assert!(validate_license_key("hyg_1234-ABCD").is_ok());
        assert!(validate_license_key("short").is_err());
        assert!(validate_license_key("hyg bad key").is_err());
    }

    #[test]
    fn license_ids_are_strict_and_account_safe() {
        assert!(validate_license_id("lic_123.alpha").is_ok());
        assert!(validate_license_id("license/id").is_err());
        assert!(validate_license_id(" ").is_err());
    }
}
