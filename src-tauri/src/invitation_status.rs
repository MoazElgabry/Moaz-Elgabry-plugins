use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MARKER_FILE: &str = "development-invitation.status.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvitationStatus {
    Connected,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationStatusMarker {
    pub version: u8,
    pub status: InvitationStatus,
    pub updated_at: u64,
}

impl InvitationStatusMarker {
    pub fn new(status: InvitationStatus) -> Self {
        Self {
            version: 1,
            status,
            updated_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

pub fn marker_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .context("No local or roaming application data directory is available")?;
    Ok(base
        .join("Moaz Elgabry Plugins")
        .join("access")
        .join(MARKER_FILE))
}

pub fn write(status: InvitationStatus) -> Result<()> {
    write_to_path(&marker_path()?, &InvitationStatusMarker::new(status))
}

pub fn write_to_path(path: &Path, marker: &InvitationStatusMarker) -> Result<()> {
    let parent = path
        .parent()
        .context("Invitation status marker path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(marker)?;
    let temp_path = parent.join(format!(".{MARKER_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("Failed to create {}", temp_path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("Failed to write {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("Failed to flush {}", temp_path.display()))?;
        atomic_replace(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

pub fn read(path: &Path) -> InvitationStatus {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<InvitationStatusMarker>(&bytes).ok())
        .filter(|marker| marker.version == 1)
        .map(|marker| marker.status)
        .unwrap_or(InvitationStatus::Unknown)
}

#[cfg(not(windows))]
fn atomic_replace(temp_path: &Path, path: &Path) -> Result<()> {
    fs::rename(temp_path, path)
        .with_context(|| format!("Failed to atomically write {}", path.display()))
}

#[cfg(windows)]
fn atomic_replace(temp_path: &Path, path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to atomically write {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_only_version_status_and_timestamp() {
        let marker = InvitationStatusMarker {
            version: 1,
            status: InvitationStatus::Connected,
            updated_at: 42,
        };
        let value = serde_json::to_value(&marker).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "version": 1,
                "status": "connected",
                "updated_at": 42
            })
        );
    }

    #[test]
    fn missing_corrupt_and_future_markers_read_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MARKER_FILE);
        assert_eq!(read(&path), InvitationStatus::Unknown);
        fs::write(&path, b"not json").unwrap();
        assert_eq!(read(&path), InvitationStatus::Unknown);
        fs::write(
            &path,
            br#"{"version":2,"status":"connected","updated_at":1}"#,
        )
        .unwrap();
        assert_eq!(read(&path), InvitationStatus::Unknown);
    }

    #[test]
    fn writes_marker_atomically_at_requested_access_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("Moaz Elgabry Plugins")
            .join("access")
            .join(MARKER_FILE);
        let marker = InvitationStatusMarker {
            version: 1,
            status: InvitationStatus::Disconnected,
            updated_at: 123,
        };
        write_to_path(&path, &marker).unwrap();
        assert_eq!(read(&path), InvitationStatus::Disconnected);
        assert_eq!(
            serde_json::from_slice::<InvitationStatusMarker>(&fs::read(&path).unwrap()).unwrap(),
            marker
        );
        write_to_path(
            &path,
            &InvitationStatusMarker {
                status: InvitationStatus::Connected,
                updated_at: 124,
                ..marker
            },
        )
        .unwrap();
        assert_eq!(read(&path), InvitationStatus::Connected);
    }
}
