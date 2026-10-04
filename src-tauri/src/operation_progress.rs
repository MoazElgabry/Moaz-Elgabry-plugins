use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const OPERATION_PROGRESS_EVENT: &str = "operation-progress";

#[derive(Clone)]
pub struct OperationProgressReporter {
    app: AppHandle,
    operation_id: String,
    plugin_id: Option<String>,
}

impl OperationProgressReporter {
    pub fn new(app: AppHandle, operation_id: impl Into<String>, plugin_id: Option<String>) -> Self {
        Self {
            app,
            operation_id: operation_id.into(),
            plugin_id,
        }
    }

    pub fn update(&self, percent: Option<u8>, label: impl Into<String>, detail: Option<String>) {
        let _ = self.app.emit(
            OPERATION_PROGRESS_EVENT,
            OperationProgressEvent {
                operation_id: self.operation_id.clone(),
                plugin_id: self.plugin_id.clone(),
                percent: percent.map(|value| value.min(100)),
                label: label.into(),
                detail,
            },
        );
    }

    pub fn transfer(
        &self,
        completed_bytes: u64,
        total_bytes: Option<u64>,
        percent_start: u8,
        percent_end: u8,
        label: &str,
    ) {
        let (percent, detail) = match total_bytes.filter(|total| *total > 0) {
            Some(total) => {
                let completed = completed_bytes.min(total);
                let fraction = completed as f64 / total as f64;
                let percent = percent_start as f64
                    + (percent_end.saturating_sub(percent_start)) as f64 * fraction;
                let remaining = total.saturating_sub(completed);
                (
                    Some(percent.round() as u8),
                    Some(format!(
                        "{} of {} downloaded · {} remaining",
                        format_bytes(completed),
                        format_bytes(total),
                        format_bytes(remaining)
                    )),
                )
            }
            None => (
                None,
                Some(format!(
                    "{} downloaded · total size unavailable",
                    format_bytes(completed_bytes)
                )),
            ),
        };
        self.update(percent, label, detail);
    }

    pub async fn download_response(
        &self,
        mut response: reqwest::Response,
        percent_start: u8,
        percent_end: u8,
        label: &str,
    ) -> Result<Vec<u8>> {
        let total_bytes = response.content_length();
        let mut bytes = Vec::new();
        let mut last_reported_percent = None;
        let mut last_reported_bytes = 0_u64;
        self.transfer(0, total_bytes, percent_start, percent_end, label);

        while let Some(chunk) = response
            .chunk()
            .await
            .context("Failed while reading the downloaded package")?
        {
            bytes.extend_from_slice(&chunk);
            let downloaded = bytes.len() as u64;
            let current_percent = total_bytes
                .filter(|total| *total > 0)
                .map(|total| {
                    let fraction = downloaded.min(total) as f64 / total as f64;
                    (percent_start as f64
                        + percent_end.saturating_sub(percent_start) as f64 * fraction)
                        .round() as u8
                });
            let should_report = match current_percent {
                Some(percent) => last_reported_percent != Some(percent),
                None => downloaded.saturating_sub(last_reported_bytes) >= 512 * 1024,
            };
            if should_report {
                self.transfer(downloaded, total_bytes, percent_start, percent_end, label);
                last_reported_percent = current_percent;
                last_reported_bytes = downloaded;
            }
        }

        self.transfer(bytes.len() as u64, total_bytes, percent_start, percent_end, label);
        Ok(bytes)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationProgressEvent {
    operation_id: String,
    plugin_id: Option<String>,
    percent: Option<u8>,
    label: String,
    detail: Option<String>,
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut amount = bytes as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit < UNITS.len() - 1 {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}
