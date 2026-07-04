use std::sync::Arc;

use crate::types::{ColorMode, Config, PrintAttributes, Printer};
use futures::io::Cursor;
use ipp::prelude::*;
use tokio_util::bytes::Bytes;

pub struct PrinterManager {
    printers: Vec<Printer>,
    color_counter: usize,
    monochrome_counter: usize,
    order_counter: usize,
}

impl PrinterManager {
    pub fn new(printers: Vec<Printer>) -> Self {
        Self {
            printers,
            color_counter: 0,
            monochrome_counter: 0,
            order_counter: 0,
        }
    }

    pub fn get_printers_for_order(
        &mut self,
        has_color: bool,
        has_mono: bool,
    ) -> (Option<Printer>, Option<Printer>, Option<String>) {
        self.order_counter += 1;

        let color_printer = if has_color {
            let color_printers: Vec<_> = self
                .printers
                .iter()
                .filter(|p| p.color_mode == ColorMode::Color)
                .collect();
            if !color_printers.is_empty() {
                let p = color_printers[self.color_counter % color_printers.len()].clone();
                self.color_counter += 1;
                Some(p)
            } else {
                None
            }
        } else {
            None
        };

        let mono_printer = if has_mono {
            let mono_printers: Vec<_> = self
                .printers
                .iter()
                .filter(|p| p.color_mode == ColorMode::Monochrome)
                .collect();
            if !mono_printers.is_empty() {
                let p = mono_printers[self.monochrome_counter % mono_printers.len()].clone();
                self.monochrome_counter += 1;
                Some(p)
            } else {
                None
            }
        } else {
            None
        };

        let media_source = if self.order_counter > 2 {
            if self.order_counter % 2 == 1 {
                Some("cas-1".to_string())
            } else {
                Some("cas-2".to_string())
            }
        } else {
            None
        };

        (color_printer, mono_printer, media_source)
    }

    pub fn get_printer(&mut self, color_mode: &ColorMode) -> Option<Printer> {
        let color_mode_printers: Vec<_> = self
            .printers
            .iter()
            .filter(|p| p.color_mode == *color_mode)
            .collect();

        if color_mode_printers.is_empty() {
            return None;
        }

        match color_mode {
            ColorMode::Color => {
                let printer = color_mode_printers[self.color_counter % color_mode_printers.len()];
                self.color_counter += 1;
                Some(printer.clone())
            }
            ColorMode::Monochrome => {
                let printer =
                    color_mode_printers[self.monochrome_counter % color_mode_printers.len()];
                self.monochrome_counter += 1;
                Some(printer.clone())
            }
        }
    }
}

pub async fn print_job(
    printer_uri: Uri,
    printer_name: String,
    attributes: PrintAttributes,
    media_source: Option<String>,
    config: Arc<Config>,
    http_client: Arc<reqwest::Client>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = download_file(attributes.file_id.clone(), &config, &http_client).await?;
    let payload = IppPayload::new_async(file);

    let print_job = IppOperationBuilder::print_job(printer_uri.clone(), payload)
        .attributes(build_ipp_attributes(attributes.clone(), media_source))
        .build();

    let response = AsyncIppClient::new(printer_uri.clone()).send(print_job).await?;

    let mut job_id = None;
    if let Some(group) = response.attributes().groups_of(DelimiterTag::JobAttributes).next() {
        if let Some(attr) = group.attributes().get("job-id") {
            if let Some(&id) = attr.value().as_integer() {
                job_id = Some(id);
            }
        }
    }

    if let Some(job_id) = job_id {
        log::info!("Job ID: {}, starting status polling...", job_id);

        let ipp_client = AsyncIppClient::new(printer_uri.clone());
        let mut pending_seconds = 0;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            let get_attrs = IppOperationBuilder::get_job_attributes(printer_uri.clone(), job_id).build();
            match ipp_client.send(get_attrs).await {
                Ok(resp) => {
                    let mut job_state = None;
                    if let Some(group) = resp.attributes().groups_of(DelimiterTag::JobAttributes).next() {
                        if let Some(attr) = group.attributes().get("job-state") {
                            if let Some(&state) = attr.value().as_enum() {
                                job_state = Some(state);
                            }
                        }
                    }

                    if let Some(state) = job_state {
                        log::info!("Job {} state: {}", job_id, state);
                        // IPP Job States:
                        // 3 = pending, 4 = pending-held, 5 = processing, 6 = processing-stopped
                        // 7 = canceled, 8 = aborted, 9 = completed
                        if state == 9 {
                            log::info!("Job {} completed successfully", job_id);
                            if let Err(e) = notify_webhook(&attributes.file_id, &printer_name, &config, &http_client).await {
                                log::error!("Failed to notify webhook: {}", e);
                            }
                            return Ok(());
                        } else if state == 7 || state == 8 {
                            return Err(format!("Job {} was canceled or aborted (state: {})", job_id, state).into());
                        } else if state == 3 {
                            pending_seconds += 1;
                            if pending_seconds >= 30 {
                                log::warn!("Job {} stuck in pending for 30s, returning PendingTimeout", job_id);
                                let cancel_op = IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build();
                                let _ = ipp_client.send(cancel_op).await;
                                return Err("PendingTimeout: Job stuck in pending state for 30 seconds".into());
                            }
                        } else {
                            pending_seconds = 0;
                        }
                    } else {
                        log::warn!("Could not retrieve job state for job {}", job_id);
                    }
                }
                Err(e) => {
                    return Err(format!("Error polling job status for job {}: {}", job_id, e).into());
                }
            }
        }
    } else {
        return Err("No job ID received in print response".into());
    }
}

async fn notify_webhook(
    file_id: &str,
    printer_name: &str,
    config: &Config,
    http_client: &reqwest::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref webhook_url) = config.webhook_url {
        log::info!("Notifying webhook: {} for file: {}", webhook_url, file_id);

        let payload = serde_json::json!({
            "event": "print.completed",
            "id": file_id,
            "printerName": printer_name
        });

        let mut req = http_client.post(webhook_url).json(&payload);
        if let Some(ref key) = config.printf_key {
            req = req.header("X-Printf-Key", key.as_str());
        }
        let response = req.send().await?;

        if !response.status().is_success() {
            return Err(format!("Webhook returned status code: {}", response.status()).into());
        }
        log::info!("Webhook notified successfully");
    } else {
        log::info!("No webhook URL configured; skipping notification.");
    }
    Ok(())
}

async fn download_file(
    file_id: String,
    config: &Config,
    http_client: &reqwest::Client,
) -> Result<Cursor<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
    let file_url = format!("{}{}", config.s3_base_url, file_id);
    let response = http_client.get(&file_url).send().await?;
    let bytes = response.bytes().await?;
    Ok(Cursor::new(bytes))
}

fn build_ipp_attributes(attributes: PrintAttributes, media_source: Option<String>) -> Vec<IppAttribute> {
    let mut attrs: Vec<IppAttribute> = [
        ("orientation-requested", attributes.orientation),
        ("print-color-mode", attributes.color.to_val().to_string()),
        ("copies", attributes.copies),
        ("media", attributes.paper_format),
        ("page-ranges", attributes.page_ranges),
        ("number-up", attributes.number_up),
        ("sides", attributes.sides),
        ("print-scaling", attributes.print_scaling),
        ("document-format", "application/octet-stream".to_string()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .filter_map(|(name, value)| match value.parse() {
        Ok(v) => Some(IppAttribute::new(name, v)),
        Err(e) => {
            log::warn!("Skipping IPP attribute '{}' with value '{}': {}", name, value, e);
            None
        }
    })
    .collect();

    if let Some(source) = media_source {
        use std::collections::BTreeMap;
        let media_col = IppValue::Collection(BTreeMap::from([
            ("media-source".to_string(), IppValue::Keyword(source))
        ]));
        attrs.push(IppAttribute::new("media-col", media_col));
    }

    attrs
}

pub async fn get_ipp_printers() -> Result<Vec<Printer>, Box<dyn std::error::Error + Send + Sync>> {
    let client = AsyncIppClient::builder("http://localhost:631".parse()?).build();
    let operation = IppOperationBuilder::cups().get_printers();
    let result = client.send(operation).await?;

    let mut printers: Vec<Printer> = Vec::new();

    for group in result
        .attributes()
        .groups_of(DelimiterTag::PrinterAttributes)
    {
        let color_mode = group.attributes()["color-supported"]
            .value()
            .as_boolean()
            .map(|is_color| match is_color {
                true => ColorMode::Color,
                false => ColorMode::Monochrome,
            })
            .unwrap();

        let uri = group.attributes()["printer-uri-supported"]
            .value()
            .to_string();

        let name = group
            .attributes()
            .get("printer-name")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| uri.clone());

        printers.push(Printer { uri, name, color_mode });
    }

    println!("{:#?}", printers);

    Ok(printers)
}
