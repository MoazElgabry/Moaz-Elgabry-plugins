//! Product access, signed receipts, and per-product device activation.
//!
//! Paid access is deliberately independent from development invitations. A
//! license key can return several product entitlements, and each entitlement
//! is persisted under the response's stable license id so overlapping keys
//! remain independently manageable.

use crate::credentials;
use crate::models::{
    AccessOperationResult, AccessProductResult, AccessProductStatus, AccessState,
    DevelopmentAccessStatus, LicenseSourceStatus,
};
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use machine_uid::get as machine_uid;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const ACCESS_API_BASE: &str = "https://moazelgabry.com/wp-json/moaz-releases/v1";
pub const HYOGEN_DEV_BUNDLE_IDENTIFIER: &str = "com.moazelgabry.hyogen.dev";
pub const HYOGEN_MODULES_BUNDLE_IDENTIFIER: &str = "com.moazelgabry.hyogen.modules";
const HYOGEN_PRODUCT_SLUG: &str = "hyogen";

pub const HYOGEN_ACCESS_PUBLIC_KEY_BASE64URL: &str =
    match option_env!("MOAZ_ER_ACCESS_PUBLIC_KEY_BASE64URL") {
        Some(key) => key,
        None => "",
    };

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEnvelope {
    pub payload: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct ReceiptPayload {
    pub version: u32,
    pub product_slug: String,
    pub device_id: String,
    pub issued_at: i64,
    pub expires_at: i64,
    #[serde(default)]
    pub license_id: Option<serde_json::Value>,
    #[serde(default)]
    pub development_bundle: Option<String>,
    #[serde(default)]
    pub development_bundles: Option<Vec<String>>,
    #[serde(default)]
    pub grants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
struct ProductResponse {
    product_slug: String,
    status: String,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    receipt: Option<ReceiptEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
struct AccessResponse {
    license_id: FlexibleLicenseId,
    #[serde(default)]
    email: Option<String>,
    products: Vec<ProductResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum FlexibleLicenseId {
    String(String),
    Number(u64),
}

impl FlexibleLicenseId {
    fn as_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Number(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct AccessRequest<'a> {
    key: &'a str,
    device_id: &'a str,
    product_slug: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProductAccess {
    license_id: String,
    #[serde(default)]
    email: Option<String>,
    product_slug: String,
    status: String,
    expires_at: Option<i64>,
    receipt: Option<ReceiptEnvelope>,
}

#[derive(Debug, Clone)]
struct VerifiedStoredProduct {
    stored: StoredProductAccess,
    message: Option<String>,
}

pub fn status() -> Result<AccessState> {
    let device_id = device_id()?;
    let mut records = read_stored_records(&device_id)?;
    match read_legacy_hyogen_receipt(&device_id, false) {
        Ok(Some(legacy)) => records.push(VerifiedStoredProduct {
            stored: StoredProductAccess {
                license_id: "legacy-hyogen".to_string(),
                email: None,
                product_slug: HYOGEN_PRODUCT_SLUG.to_string(),
                status: if legacy.expires_at > unix_now()? { "active".to_string() } else { "expired".to_string() },
                expires_at: Some(legacy.expires_at),
                receipt: Some(legacy.envelope),
            },
            message: None,
        }),
        Ok(None) => {}
        Err(error) => records.push(VerifiedStoredProduct {
            stored: StoredProductAccess {
                license_id: "legacy-hyogen".to_string(),
                email: None,
                product_slug: HYOGEN_PRODUCT_SLUG.to_string(),
                status: "invalid".to_string(),
                expires_at: None,
                receipt: None,
            },
            message: Some(error.to_string()),
        }),
    }
    Ok(build_state(device_id, records))
}

/// Remove cached development-invitation receipts without touching paid
/// product-license receipts or their credentials.
pub fn forget_development_receipts() -> Result<()> {
    retain_development_receipts(&[])
}

/// Report whether a locally cached development receipt is currently trusted
/// and unexpired. Invitation connectivity is intentionally not required here:
/// a valid receipt remains usable during the offline grace period.
pub fn development_receipt_status(product_slug: &str) -> Result<String> {
    let product_slug = valid_product_slug(product_slug)?;
    let device_id = device_id()?;
    let path = legacy_receipt_path(product_slug, true)?;
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("inactive".to_string());
        }
        Err(error) => return Err(error).with_context(|| format!("Failed to read {}", path.display())),
    };
    let envelope = serde_json::from_str::<ReceiptEnvelope>(&raw)
        .context("The stored development receipt is invalid JSON")?;
    let payload = verified_payload(&envelope, &device_id, Some(product_slug), None, true, true)?;
    Ok(if payload.expires_at > unix_now()? {
        "active".to_string()
    } else {
        "expired".to_string()
    })
}

/// Keep only receipts for products that the currently connected invitation
/// still grants. This is called after a successful online catalog refresh;
/// network failures do not reach this path, preserving offline access.
pub fn retain_development_receipts(allowed_products: &[String]) -> Result<()> {
    let directory = access_dir()?;
    if !directory.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&directory)
        .with_context(|| format!("Failed to inspect {}", directory.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let filename = entry.file_name();
        let filename = filename.to_string_lossy();
        let Some(product_slug) = filename.strip_suffix(".development.receipt.json") else {
            continue;
        };
        if valid_product_slug(product_slug).is_err()
            || allowed_products.iter().any(|allowed| allowed == product_slug)
        {
            continue;
        }
        fs::remove_file(entry.path())
            .with_context(|| format!("Failed to clear the {product_slug} development receipt"))?;
    }
    Ok(())
}

pub async fn activate(key: &str, product_slug: &str) -> Result<AccessOperationResult> {
    let key = credentials::validate_license_key(key)?;
    let product_slug = valid_product_slug(product_slug)?;
    let response = request_access("/access/activate", key, product_slug).await?;
    let device_id = device_id()?;
    let records = persist_response(key, &response, &device_id)?;
    Ok(AccessOperationResult {
        license_id: response.license_id.as_string(),
        products: response
            .products
            .iter()
            .map(|product| AccessProductResult {
                product_slug: product.product_slug.clone(),
                status: product.status.clone(),
                expires_at: product.expires_at,
            })
            .collect(),
        state: build_state(device_id, records),
    })
}

pub async fn refresh(_product_slugs: &[String]) -> Result<AccessState> {
    let device_id = device_id()?;
    let mut ids = credentials::license_ids()?;
    if credentials::hyogen_license_key()?.is_some() && !ids.iter().any(|id| id == "legacy-hyogen") {
        ids.push("legacy-hyogen".to_string());
    }
    for license_id in ids {
        let key = if license_id == "legacy-hyogen" {
            credentials::hyogen_license_key()?.ok_or_else(|| anyhow!("The stored Hyogen license key is unavailable."))?
        } else {
            credentials::license_key(&license_id)?.ok_or_else(|| anyhow!("The stored license key is unavailable."))?
        };
        let product_slug = if license_id == "legacy-hyogen" {
            HYOGEN_PRODUCT_SLUG.to_string()
        } else {
            let covered_products = stored_product_slugs(&license_id)?;
            let Some(product_slug) = covered_product_for_refresh(&covered_products) else {
                continue;
            };
            product_slug.to_string()
        };
        let response = request_access("/access/refresh", &key, &product_slug).await?;
        let _ = persist_response(&key, &response, &device_id)?;
    }
    status()
}

pub async fn deactivate(license_id: &str, product_slug: &str) -> Result<AccessState> {
    let product_slug = valid_product_slug(product_slug)?;
    let key = if license_id == "legacy-hyogen" {
        credentials::hyogen_license_key()?.ok_or_else(|| anyhow!("The stored Hyogen license key is unavailable."))?
    } else {
        credentials::license_key(license_id)?.ok_or_else(|| anyhow!("The selected license key is unavailable."))?
    };
    let device_id = device_id()?;
    post_deactivate(&key, &device_id, product_slug).await?;
    if license_id == "legacy-hyogen" {
        remove_legacy_hyogen_receipt(false)?;
    } else {
        remove_stored_product(license_id, product_slug)?;
    }
    status()
}

/// Development invitation receipts stay separate from paid access. The
/// caller supplies the product selected by the build/install context; the
/// signed product grant and bundle set remain in the development receipt for
/// the plugin runtime to validate.
pub async fn activate_development_access(product_slug: &str) -> Result<DevelopmentAccessStatus> {
    let key = credentials::invitation_token()?.ok_or_else(|| anyhow!("No development invitation is connected."))?;
    let product_slug = valid_product_slug(product_slug)?;
    let response = request_access("/access/activate", &key, product_slug).await?;
    let product = response
        .products
        .iter()
        .find(|product| product.product_slug == product_slug)
        .ok_or_else(|| anyhow!("The development invitation did not cover this product."))?;
    let envelope = product.receipt.as_ref().ok_or_else(|| anyhow!("The development service returned no receipt."))?;
    let current_device = device_id()?;
    let payload = verified_payload(envelope, &current_device, Some(product_slug), None, true, true)?;
    if !payload.grants.iter().any(|grant| grant == &development_grant(product_slug)) {
        return Err(anyhow!("The development invitation did not grant access to this product."));
    }
    write_legacy_receipt(envelope, product_slug, true)?;
    Ok(status_from_payload(payload, &current_device))
}

async fn request_access(path: &str, key: &str, product_slug: &str) -> Result<AccessResponse> {
    let device_id = device_id()?;
    let client = Client::builder().timeout(std::time::Duration::from_secs(20)).build().context("Unable to prepare the access service connection")?;
    let response = client
        .post(format!("{ACCESS_API_BASE}{path}"))
        .json(&AccessRequest { key, device_id: &device_id, product_slug })
        .send()
        .await
        .context("The access service could not be reached")?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let code = response.json::<serde_json::Value>().await.ok().and_then(|body| body.get("code").and_then(serde_json::Value::as_str).map(str::to_owned));
        let suffix = code.map(|value| format!(" ({value})")).unwrap_or_default();
        return Err(anyhow!("The access service rejected the request with HTTP {status}{suffix}."));
    }
    response.json::<AccessResponse>().await.context("The access service returned an invalid product access response")
}

async fn post_deactivate(key: &str, device_id: &str, product_slug: &str) -> Result<()> {
    let client = Client::builder().timeout(std::time::Duration::from_secs(20)).build().context("Unable to prepare the access service connection")?;
    let response = client
        .post(format!("{ACCESS_API_BASE}/access/deactivate"))
        .json(&AccessRequest { key, device_id, product_slug })
        .send()
        .await
        .context("The access service could not be reached")?;
    if response.status().is_success() { return Ok(()); }
    let status = response.status().as_u16();
    let code = response.json::<serde_json::Value>().await.ok().and_then(|body| body.get("code").and_then(serde_json::Value::as_str).map(str::to_owned));
    let suffix = code.map(|value| format!(" ({value})")).unwrap_or_default();
    Err(anyhow!("The access service rejected the request with HTTP {status}{suffix}."))
}

fn persist_response(key: &str, response: &AccessResponse, device_id: &str) -> Result<Vec<VerifiedStoredProduct>> {
    let license_id = response.license_id.as_string();
    let license_id = credentials::validate_license_id(&license_id)?;
    for product in &response.products {
        let product_slug = valid_product_slug(&product.product_slug)?;
        let status = if product.status == "active" && product.receipt.is_none() {
            "invalid".to_string()
        } else {
            normalize_status(&product.status)
        };
        if let Some(receipt) = &product.receipt {
            verified_payload(receipt, device_id, Some(product_slug), None, false, false).with_context(|| format!("The {product_slug} receipt could not be verified"))?;
        }
        write_stored_product(&StoredProductAccess {
            license_id: license_id.to_owned(),
            email: response.email.clone(),
            product_slug: product_slug.to_owned(),
            status,
            expires_at: product.expires_at,
            receipt: product.receipt.clone(),
        })?;
    }
    credentials::store_license_key(license_id, key)?;
    read_stored_records(device_id)
}

fn read_stored_records(device_id: &str) -> Result<Vec<VerifiedStoredProduct>> {
    let dir = product_access_dir()?;
    if !dir.exists() { return Ok(Vec::new()); }
    let mut records = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") { continue; }
        let raw = match fs::read_to_string(&path) { Ok(raw) => raw, Err(_) => continue };
        let mut stored = match serde_json::from_str::<StoredProductAccess>(&raw) { Ok(stored) => stored, Err(_) => continue };
        if credentials::validate_license_id(&stored.license_id).is_err() || valid_product_slug(&stored.product_slug).is_err() { continue; }
        let mut message = None;
        if let Some(receipt) = &stored.receipt {
            match verified_payload(receipt, device_id, Some(&stored.product_slug), None, true, false) {
                Ok(payload) => {
                    stored.expires_at = Some(payload.expires_at);
                    if payload.expires_at <= unix_now()? { stored.status = "expired".to_string(); }
                    else if stored.status == "expired" { stored.status = "active".to_string(); }
                }
                Err(error) => { stored.status = "invalid".to_string(); message = Some(error.to_string()); }
            }
        }
        records.push(VerifiedStoredProduct { stored, message });
    }
    Ok(records)
}

fn stored_product_slugs(license_id: &str) -> Result<Vec<String>> {
    let dir = product_access_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut products = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let stored = match serde_json::from_str::<StoredProductAccess>(&raw) {
            Ok(stored) => stored,
            Err(_) => continue,
        };
        if stored.license_id == license_id && valid_product_slug(&stored.product_slug).is_ok() {
            products.push(stored.product_slug);
        }
    }
    products.sort();
    products.dedup();
    Ok(products)
}

fn covered_product_for_refresh(products: &[String]) -> Option<&str> {
    products.first().map(String::as_str)
}

fn build_state(device_id: String, records: Vec<VerifiedStoredProduct>) -> AccessState {
    let mut products = std::collections::BTreeMap::<String, Vec<VerifiedStoredProduct>>::new();
    for record in records { products.entry(record.stored.product_slug.clone()).or_default().push(record); }
    AccessState {
        device_id,
        products: products.into_iter().map(|(product_slug, records)| {
            let status = records.iter().map(|record| record.stored.status.as_str()).max_by_key(|status| status_rank(status)).unwrap_or("not_activated").to_string();
            let expires_at = records.iter().filter_map(|record| record.stored.expires_at).max();
            let message = records.iter().find_map(|record| record.message.clone());
            let licenses = records.into_iter().map(|record| LicenseSourceStatus {
                license_id: record.stored.license_id.clone(),
                email: record.stored.email,
                status: record.stored.status,
                expires_at: record.stored.expires_at,
                key_available: record.stored.license_id == "legacy-hyogen" || credentials::license_key(&record.stored.license_id).ok().flatten().is_some(),
            }).collect();
            AccessProductStatus { product_slug, status, expires_at, licenses, message }
        }).collect(),
    }
}

fn status_rank(status: &str) -> u8 {
    match status { "active" => 6, "expired" => 5, "device_limit_reached" => 4, "revoked" => 3, "invalid" => 2, _ => 1 }
}

fn normalize_status(status: &str) -> String {
    match status { "active" | "device_limit_reached" | "expired" | "revoked" => status.to_string(), _ => "unknown".to_string() }
}

fn verified_payload(envelope: &ReceiptEnvelope, device_id: &str, expected_product: Option<&str>, expected_bundle: Option<&str>, allow_expired: bool, development: bool) -> Result<ReceiptPayload> {
    let payload = verify_envelope(envelope)?;
    let expected_product = expected_product.unwrap_or(HYOGEN_PRODUCT_SLUG);
    if valid_product_slug(expected_product).is_err() || valid_product_slug(&payload.product_slug).is_err() { return Err(anyhow!("The receipt product identifier is invalid.")); }
    if payload.product_slug != expected_product { return Err(anyhow!("The receipt is for a different product.")); }
    if payload.device_id != device_id { return Err(anyhow!("The receipt belongs to a different device.")); }
    if development {
        if !has_development_access(&payload, expected_product) { return Err(anyhow!("The development receipt payload is invalid.")); }
        if let Some(expected_bundle) = expected_bundle {
            let matches = payload.development_bundles.as_ref().is_some_and(|bundles| bundles.iter().any(|bundle| bundle == expected_bundle)) || payload.development_bundle.as_deref() == Some(expected_bundle);
            if !matches { return Err(anyhow!("The receipt is not bound to the requested development bundle.")); }
        }
    } else if payload.development_bundle.is_some() || payload.development_bundles.is_some() { return Err(anyhow!("The product license receipt payload is invalid.")); }
    if payload.issued_at > payload.expires_at { return Err(anyhow!("The receipt payload is invalid.")); }
    if !allow_expired && payload.expires_at <= unix_now()? { return Err(anyhow!("The license receipt has expired.")); }
    Ok(payload)
}

fn verify_envelope(envelope: &ReceiptEnvelope) -> Result<ReceiptPayload> {
    let verifying_key = pinned_verifying_key()?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(&envelope.payload).context("The receipt payload encoding is invalid")?;
    let signature_bytes = URL_SAFE_NO_PAD.decode(&envelope.signature).context("The receipt signature encoding is invalid")?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|_| anyhow!("The receipt signature has the wrong length"))?;
    verifying_key.verify(&payload_bytes, &signature).map_err(|_| anyhow!("The receipt signature is invalid"))?;
    serde_json::from_slice(&payload_bytes).context("The receipt payload JSON is invalid")
}

fn pinned_verifying_key() -> Result<VerifyingKey> {
    if HYOGEN_ACCESS_PUBLIC_KEY_BASE64URL.is_empty() { return Err(anyhow!("Receipt verification is not configured in this build.")); }
    let bytes = URL_SAFE_NO_PAD.decode(HYOGEN_ACCESS_PUBLIC_KEY_BASE64URL).context("The pinned access public key is invalid")?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("The pinned access public key has the wrong length"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow!("The pinned access public key is invalid"))
}

fn status_from_payload(payload: ReceiptPayload, device_id: &str) -> DevelopmentAccessStatus {
    DevelopmentAccessStatus {
        product_slug: payload.product_slug,
        status: if payload.expires_at > unix_now().unwrap_or_default() { "active".to_string() } else { "expired".to_string() },
        device_id: device_id.to_owned(),
        expires_at: Some(payload.expires_at),
        development_bundle: payload.development_bundle,
        development_bundles: payload.development_bundles.unwrap_or_default(),
        grants: payload.grants,
        message: None,
    }
}

fn read_legacy_hyogen_receipt(device_id: &str, development: bool) -> Result<Option<LegacyReceipt>> {
    let path = legacy_receipt_path(HYOGEN_PRODUCT_SLUG, development)?;
    if !path.exists() { return Ok(None); }
    let raw = fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let envelope = serde_json::from_str::<ReceiptEnvelope>(&raw).context("The stored Hyogen receipt is invalid JSON")?;
    let payload = verified_payload(
        &envelope,
        device_id,
        Some(HYOGEN_PRODUCT_SLUG),
        development.then_some(HYOGEN_DEV_BUNDLE_IDENTIFIER),
        true,
        development,
    )?;
    Ok(Some(LegacyReceipt { envelope, expires_at: payload.expires_at }))
}

struct LegacyReceipt { envelope: ReceiptEnvelope, expires_at: i64 }

fn write_legacy_receipt(envelope: &ReceiptEnvelope, product_slug: &str, development: bool) -> Result<()> {
    let path = legacy_receipt_path(product_slug, development)?;
    atomic_write(&path, &serde_json::to_vec_pretty(envelope)?)
}

fn remove_legacy_hyogen_receipt(development: bool) -> Result<()> {
    match fs::remove_file(legacy_receipt_path(HYOGEN_PRODUCT_SLUG, development)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Failed to remove the legacy Hyogen receipt"),
    }
}

fn write_stored_product(stored: &StoredProductAccess) -> Result<()> {
    let metadata_path = stored_product_metadata_path(&stored.license_id, &stored.product_slug)?;
    atomic_write(&metadata_path, &serde_json::to_vec_pretty(stored)?)?;
    if let Some(receipt) = &stored.receipt {
        let receipt_path = stored_product_receipt_path(&stored.license_id, &stored.product_slug)?;
        atomic_write(&receipt_path, &serde_json::to_vec_pretty(receipt)?)?;
    } else {
        match fs::remove_file(stored_product_receipt_path(&stored.license_id, &stored.product_slug)?) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Failed to remove the stale product access receipt"),
        }
    }
    Ok(())
}

fn remove_stored_product(license_id: &str, product_slug: &str) -> Result<()> {
    match fs::remove_file(stored_product_metadata_path(license_id, product_slug)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Failed to remove the product access record"),
    }?;
    match fs::remove_file(stored_product_receipt_path(license_id, product_slug)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Failed to remove the product access receipt"),
    }
}

fn access_dir() -> Result<PathBuf> {
    dirs::data_local_dir().or_else(dirs::data_dir).map(|base| base.join("Moaz Elgabry Plugins").join("access")).ok_or_else(|| anyhow!("Unable to resolve local application data directory"))
}

fn product_access_dir() -> Result<PathBuf> { Ok(access_dir()?.join("products")) }

fn legacy_receipt_path(product_slug: &str, development: bool) -> Result<PathBuf> {
    valid_product_slug(product_slug)?;
    Ok(access_dir()?.join(if development { format!("{product_slug}.development.receipt.json") } else { format!("{product_slug}.receipt.json") }))
}

fn stored_product_metadata_path(license_id: &str, product_slug: &str) -> Result<PathBuf> {
    credentials::validate_license_id(license_id)?;
    valid_product_slug(product_slug)?;
    let mut hasher = Sha256::new();
    hasher.update(license_id.as_bytes());
    let digest = hex::encode(hasher.finalize());
    Ok(product_access_dir()?.join(format!("{product_slug}.{digest}.json")))
}

fn stored_product_receipt_path(license_id: &str, product_slug: &str) -> Result<PathBuf> {
    credentials::validate_license_id(license_id)?;
    valid_product_slug(product_slug)?;
    Ok(access_dir()?.join(format!("{product_slug}.{license_id}.receipt.json")))
}

fn valid_product_slug(product_slug: &str) -> Result<&str> {
    let product_slug = product_slug.trim();
    if product_slug.is_empty() || product_slug.len() > 100 || !product_slug.bytes().enumerate().all(|(index, byte)| byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')) { return Err(anyhow!("The product identifier is invalid.")); }
    Ok(product_slug)
}

fn development_grant(product_slug: &str) -> String { format!("development_download:{product_slug}:dev") }

fn has_development_access(payload: &ReceiptPayload, product_slug: &str) -> bool {
    let has_grant = payload.grants.iter().any(|value| value == &development_grant(product_slug));
    let has_bundle_set = payload.development_bundles.as_ref().is_some_and(|bundles| !bundles.is_empty())
        || payload.development_bundle.as_ref().is_some_and(|bundle| !bundle.trim().is_empty());
    payload.product_slug == product_slug && has_grant && has_bundle_set
}

fn device_id() -> Result<String> {
    let raw = machine_uid().map_err(|error| anyhow!("Unable to resolve the OS machine identifier: {error}"))?;
    if raw.trim().is_empty() { return Err(anyhow!("The OS machine identifier is empty")); }
    let mut hasher = Sha256::new();
    hasher.update(b"moaz-access-device-v1:");
    hasher.update(raw.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

fn unix_now() -> Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).context("System clock is before the Unix epoch")?.as_secs() as i64)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!("The access path has no parent directory"))?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let temp_path = parent.join(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), Uuid::new_v4()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temp_path)?;
    set_user_only_permissions(&file)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    atomic_replace(&temp_path, path)
}

fn set_user_only_permissions(file: &File) -> Result<()> {
    let _ = file;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; file.set_permissions(fs::Permissions::from_mode(0o600))?; }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(temp_path: &Path, path: &Path) -> Result<()> { fs::rename(temp_path, path).with_context(|| format!("Failed to atomically write {}", path.display())) }

#[cfg(windows)]
fn atomic_replace(temp_path: &Path, path: &Path) -> Result<()> {
    if path.exists() { fs::remove_file(path).with_context(|| format!("Failed to replace {}", path.display()))?; }
    fs::rename(temp_path, path).with_context(|| format!("Failed to atomically write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn product_slugs_are_lowercase_and_path_safe() {
        assert!(valid_product_slug("hyogen").is_ok());
        assert!(valid_product_slug("me-open_drt").is_err());
        assert!(valid_product_slug("../other").is_err());
    }
    #[test]
    fn access_status_precedence_keeps_active_entitlement() {
        assert!(status_rank("active") > status_rank("expired"));
        assert!(status_rank("expired") > status_rank("device_limit_reached"));
    }
    #[test]
    fn request_shape_includes_product_scope() {
        let body = serde_json::to_value(AccessRequest { key: "key", device_id: "device", product_slug: "hyogen" }).unwrap();
        assert_eq!(body["product_slug"], "hyogen");
        assert_eq!(body["device_id"], "device");
    }

    #[test]
    fn license_id_accepts_wordpress_numeric_or_string_values() {
        let numeric: AccessResponse = serde_json::from_value(serde_json::json!({
            "license_id": 42,
            "products": []
        })).unwrap();
        assert_eq!(numeric.license_id.as_string(), "42");
        let textual: AccessResponse = serde_json::from_value(serde_json::json!({
            "license_id": "license-42",
            "products": []
        })).unwrap();
        assert_eq!(textual.license_id.as_string(), "license-42");
    }

    #[test]
    fn refresh_uses_a_saved_covered_product_instead_of_unrelated_catalog_products() {
        let saved_products = vec!["chromaspace".to_string()];
        let catalog_products = ["hyogen".to_string(), "chromaspace".to_string(), "lensdiff".to_string()];
        let target = covered_product_for_refresh(&saved_products);
        assert_eq!(target, Some("chromaspace"));
        assert!(catalog_products.iter().any(|product| product == "lensdiff"));
        assert_ne!(target, Some("lensdiff"));
    }

    #[test]
    fn development_access_accepts_a_non_hyogen_product_grant() {
        let payload = ReceiptPayload {
            version: 1,
            product_slug: "chromaspace".to_string(),
            device_id: "device".to_string(),
            issued_at: 1,
            expires_at: 2,
            license_id: None,
            development_bundle: None,
            development_bundles: Some(vec!["com.example.chromaspace.dev".to_string()]),
            grants: vec![development_grant("chromaspace")],
        };
        assert!(has_development_access(&payload, "chromaspace"));
        assert!(!has_development_access(&payload, HYOGEN_PRODUCT_SLUG));
    }
}
