use crate::{
    read_config,
    types::{ColorMode, PrintAttributes, Printer},
};
use futures::io::Cursor;
use ipp::prelude::*;
use reqwest;
use tokio_util::bytes::Bytes;

pub struct PrinterManager {
    printers: Vec<Printer>,
    color_counter: usize,
    monochrome_counter: usize,
}

impl PrinterManager {
    pub fn new(printers: Vec<Printer>) -> Self {
        Self {
            printers,
            color_counter: 0,
            monochrome_counter: 0,
        }
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
    attributes: PrintAttributes,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = download_file(attributes.file_id.clone()).await?;
    let payload = IppPayload::new_async(file);

    let print_job = IppOperationBuilder::print_job(printer_uri.clone(), payload)
        .attributes(build_ipp_attributes(attributes.clone()))
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

        let client = AsyncIppClient::new(printer_uri.clone());
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            let get_attrs = IppOperationBuilder::get_job_attributes(printer_uri.clone(), job_id).build();
            match client.send(get_attrs).await {
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
                            if let Err(e) = notify_webhook(&attributes.file_id).await {
                                log::error!("Failed to notify webhook: {}", e);
                            }
                            break;
                        } else if state == 7 || state == 8 {
                            log::error!("Job {} was canceled or aborted (state: {})", job_id, state);
                            break;
                        }
                    } else {
                        log::warn!("Could not retrieve job state for job {}", job_id);
                    }
                }
                Err(e) => {
                    log::error!("Error polling job status for job {}: {}", job_id, e);
                }
            }
        }
    } else {
        log::warn!("No job ID received in print response; skipping status polling.");
    }

    Ok(())
}

async fn notify_webhook(file_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = read_config()?;
    if let Some(ref webhook_url) = config.webhook_url {
        log::info!("Notifying webhook: {} for file: {}", webhook_url, file_id);

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "event": "print.completed",
            "id": file_id
        });

        let response = client.post(webhook_url)
            .json(&payload)
            .send()
            .await?;

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
) -> Result<Cursor<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
    let base_url = read_config()?.s3_base_url;
    let file_url = format!("{}{}", base_url, file_id);
    let response = reqwest::get(file_url).await?;
    let bytes = response.bytes().await?;
    Ok(Cursor::new(bytes))
}

fn build_ipp_attributes(attributes: PrintAttributes) -> Vec<IppAttribute> {
    [
        ("orientation-requested", attributes.orientation),
        ("print-color-mode", attributes.color.to_val().to_string()),
        ("copies", attributes.copies),
        ("media", attributes.paper_format),
        ("page-ranges", attributes.page_ranges),
        ("number-up", attributes.number_up),
        ("sides", attributes.sides),
        ("document-format", attributes.document_format),
        ("print-scaling", attributes.print_scaling),
    ]
    .into_iter()
    .map(|(name, value)| IppAttribute::new(name, value.parse().unwrap()))
    .collect()
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

        printers.push(Printer { uri, color_mode });
    }

    Ok(printers)
}
