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

    pub fn get_printers(&self) -> Vec<Printer> {
        self.printers.clone()
    }

    pub fn set_printer_paused(&mut self, uri: &str, paused: bool) {
        if let Some(p) = self.printers.iter_mut().find(|p| p.uri == uri) {
            p.paused = paused;
        }
    }

    pub fn set_printer_properties(
        &mut self,
        name: &str,
        properties: crate::types::PrinterProperties,
        color_mode: ColorMode,
    ) {
        if let Some(p) = self.printers.iter_mut().find(|p| p.name == name || p.uri.contains(name)) {
            p.properties = Some(properties);
            p.color_mode = color_mode;
        }
    }

    pub fn get_printers_for_order(
        &mut self,
        has_color: bool,
        has_mono: bool,
    ) -> (Option<Printer>, Option<Printer>, Option<String>, Option<String>) {
        self.order_counter += 1;

        let color_printer = if has_color {
            let color_printers: Vec<_> = self
                .printers
                .iter()
                .filter(|p| p.color_mode == ColorMode::Color && !p.paused)
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
                .filter(|p| p.color_mode == ColorMode::Monochrome && !p.paused)
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

        let color_media = None;
        let mono_media = if self.order_counter % 2 == 1 {
            Some("tray-1".to_string())
        } else {
            Some("tray-4".to_string())
        };

        (color_printer, mono_printer, color_media, mono_media)
    }

    pub fn get_printer(&mut self, color_mode: &ColorMode) -> Option<Printer> {
        let color_mode_printers: Vec<_> = self
            .printers
            .iter()
            .filter(|p| p.color_mode == *color_mode && !p.paused)
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
        let mut pending_seconds = 0i32;
        let mut processing_seconds = 0i32;       // state 6: processing-stopped
        let mut processing_active_seconds = 0i32; // state 5: actively processing

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            let get_attrs = IppOperationBuilder::get_job_attributes(printer_uri.clone(), job_id).build();
            match ipp_client.send(get_attrs).await {
                Ok(resp) => {
                    let mut job_state = None;
                    let mut reasons: Vec<String> = Vec::new();
                    let mut message: Option<String> = None;

                    if let Some(group) = resp.attributes().groups_of(DelimiterTag::JobAttributes).next() {
                        if let Some(attr) = group.attributes().get("job-state") {
                            if let Some(&state) = attr.value().as_enum() {
                                job_state = Some(state);
                            }
                        }
                        if let Some(attr) = group.attributes().get("job-state-reasons") {
                            match attr.value() {
                                IppValue::Keyword(k) => reasons.push(k.clone()),
                                IppValue::Array(arr) => {
                                    for item in arr {
                                        if let IppValue::Keyword(k) = item {
                                            reasons.push(k.clone());
                                        } else {
                                            reasons.push(item.to_string());
                                        }
                                    }
                                }
                                v => reasons.push(v.to_string()),
                            }
                        }
                        if let Some(attr) = group.attributes().get("job-state-message") {
                            message = Some(attr.value().to_string());
                        }
                    }

                    if let Some(state) = job_state {
                        let reasons_str = reasons.join(", ");
                        log::info!(
                            "Job {} state: {} | reasons: [{}] | msg: {:?}",
                            job_id,
                            state,
                            reasons_str,
                            message
                        );

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
                            // Canceled or aborted — treat as immediate failure/stuck
                            let err_detail = message.or_else(|| if !reasons.is_empty() { Some(reasons_str) } else { None })
                                .unwrap_or_else(|| format!("state {}", state));
                            log::warn!("Job {} was canceled or aborted: {}", job_id, err_detail);
                            return Err(format!("PendingTimeout: Job was canceled or aborted ({})", err_detail).into());
                        }

                        // Check for explicit failure/error reasons on job-state-reasons
                        let has_explicit_error = reasons.iter().any(|r| {
                            let lr = r.to_lowercase();
                            lr.contains("error")
                                || lr.contains("aborted")
                                || lr.contains("canceled")
                                || lr.contains("failed")
                                || lr.contains("stopped")
                                || lr.contains("media-jam")
                                || lr.contains("offline")
                                || lr.contains("tray-missing")
                                || lr.contains("toner-empty")
                                || lr.contains("door-open")
                        });

                        if has_explicit_error || (state == 6 && (!reasons.is_empty() || message.is_some())) {
                            let err_detail = message.or_else(|| if !reasons.is_empty() { Some(reasons_str) } else { None })
                                .unwrap_or_else(|| format!("processing-stopped (state {})", state));
                            log::warn!("Job {} failed with explicit error: {}", job_id, err_detail);
                            let cancel_op = IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build();
                            let _ = ipp_client.send(cancel_op).await;
                            return Err(format!("PendingTimeout: Job error ({})", err_detail).into());
                        }

                        if state == 3 || state == 4 {
                            // Pending / pending-held
                            pending_seconds += 1;
                            processing_seconds = 0;
                            if pending_seconds >= 30 {
                                log::warn!("Job {} stuck in pending for 30s, returning PendingTimeout", job_id);
                                let cancel_op = IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build();
                                let _ = ipp_client.send(cancel_op).await;
                                return Err("PendingTimeout: Job stuck in pending state for 30 seconds".into());
                            }
                        } else if state == 6 {
                            // Processing-stopped (without explicit error reason)
                            processing_seconds += 1;
                            pending_seconds = 0;
                            processing_active_seconds = 0;
                            if processing_seconds >= 15 {
                                log::warn!("Job {} stuck in processing-stopped for 15s, returning PendingTimeout", job_id);
                                let cancel_op = IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build();
                                let _ = ipp_client.send(cancel_op).await;
                                return Err("PendingTimeout: Job stuck in processing-stopped state for 15 seconds".into());
                            }
                        } else if state == 5 {
                            // Actively processing — allow up to 120s before giving up
                            processing_active_seconds += 1;
                            pending_seconds = 0;
                            processing_seconds = 0;
                            if processing_active_seconds >= 120 {
                                log::warn!("Job {} processing for 120s without completing, returning PendingTimeout", job_id);
                                let cancel_op = IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build();
                                let _ = ipp_client.send(cancel_op).await;
                                return Err("PendingTimeout: Job processing for 120 seconds without completing".into());
                            }
                        } else {
                            pending_seconds = 0;
                            processing_seconds = 0;
                            processing_active_seconds = 0;
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
    let mut copies_count = 1;

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
    .filter_map(|(name, value)| match name {
        "copies" => match value.parse::<i32>() {
            Ok(val) => {
                copies_count = val;
                Some(IppAttribute::new(name, IppValue::Integer(val)))
            }
            Err(e) => {
                log::warn!("Skipping integer IPP attribute '{}' with value '{}': {}", name, value, e);
                None
            }
        },
        "number-up" => match value.parse::<i32>() {
            Ok(val) => Some(IppAttribute::new(name, IppValue::Integer(val))),
            Err(e) => {
                log::warn!("Skipping integer IPP attribute '{}' with value '{}': {}", name, value, e);
                None
            }
        },
        "orientation-requested" => match value.parse::<i32>() {
            Ok(val) => Some(IppAttribute::new(name, IppValue::Enum(val))),
            Err(e) => {
                log::warn!("Skipping enum IPP attribute '{}' with value '{}': {}", name, value, e);
                None
            }
        },
        _ => match value.parse() {
            Ok(v) => Some(IppAttribute::new(name, v)),
            Err(e) => {
                log::warn!("Skipping IPP attribute '{}' with value '{}': {}", name, value, e);
                None
            }
        },
    })
    .collect();

    if copies_count > 1 {
        attrs.push(IppAttribute::new(
            "multiple-document-handling",
            IppValue::Keyword("separate-documents-uncollated-copies".to_string()),
        ));
    }

    if let Some(source) = media_source {
        use std::collections::BTreeMap;
        let media_col = IppValue::Collection(BTreeMap::from([
            ("media-source".to_string(), IppValue::Keyword(source))
        ]));
        attrs.push(IppAttribute::new("media-col", media_col));
    }

    attrs
}

fn format_ipp_uri(path: &str, creds: Option<(&str, &str)>) -> String {
    if let Some((u, p)) = creds {
        if !u.is_empty() && !p.is_empty() {
            format!("http://{}:{}@localhost:631{}", u, p, path)
        } else {
            format!("http://localhost:631{}", path)
        }
    } else {
        format!("http://localhost:631{}", path)
    }
}

pub async fn get_ipp_printers(creds: Option<(&str, &str)>) -> Result<Vec<Printer>, Box<dyn std::error::Error + Send + Sync>> {
    let uri_str = format_ipp_uri("", creds);
    let uri: Uri = uri_str.parse()?;
    let client = AsyncIppClient::builder(uri).build();
    let operation = IppOperationBuilder::cups().get_printers();
    let result = client.send(operation).await?;

    let mut printers: Vec<Printer> = Vec::new();

    for group in result
        .attributes()
        .groups_of(DelimiterTag::PrinterAttributes)
    {
        let color_mode = group
            .attributes()
            .get("color-supported")
            .and_then(|attr| attr.value().as_boolean())
            .map(|is_color| match is_color {
                true => ColorMode::Color,
                false => ColorMode::Monochrome,
            })
            .unwrap_or(ColorMode::Color);

        let uri = group
            .attributes()
            .get("printer-uri-supported")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let name = group
            .attributes()
            .get("printer-name")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| {
                uri.split('/').last().unwrap_or("Printer").to_string()
            });

        let media_default = group
            .attributes()
            .get("media-default")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| "iso_a4_210x297mm".to_string());

        let media_source_default = group
            .attributes()
            .get("media-source-default")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| "auto".to_string());

        let orientation_default = group
            .attributes()
            .get("orientation-requested-default")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| "portrait".to_string());

        let sides_default = group
            .attributes()
            .get("sides-default")
            .map(|attr| attr.value().to_string())
            .unwrap_or_else(|| "one-sided".to_string());

        let properties = Some(crate::types::PrinterProperties {
            media: media_default,
            media_source: media_source_default,
            orientation: orientation_default,
            print_quality: "normal".to_string(),
            sides: sides_default,
        });

        printers.push(Printer { uri, name, color_mode, paused: false, properties });
    }

    Ok(printers)
}

pub async fn fetch_printer_properties_via_ipp(name: &str, creds: Option<(&str, &str)>) -> (crate::types::PrinterProperties, ColorMode) {
    let mut media = "iso_a4_210x297mm".to_string();
    let mut media_source = "auto".to_string();
    let mut orientation = "portrait".to_string();
    let mut print_quality = "normal".to_string();
    let mut sides = "one-sided".to_string();
    let mut color_mode = ColorMode::Color;

    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    if let Ok(uri) = uri_str.parse::<Uri>() {
        let client = AsyncIppClient::builder(uri.clone()).build();
        let operation = IppOperationBuilder::get_printer_attributes(uri)
            .attributes(&[
                "media-default",
                "media-source-default",
                "orientation-requested-default",
                "sides-default",
                "print-color-mode-default",
                "color-supported",
            ])
            .build();

        if let Ok(response) = client.send(operation).await {
            for group in response.attributes().groups_of(DelimiterTag::PrinterAttributes) {
                if let Some(attr) = group.attributes().get("media-default") {
                    media = attr.value().to_string();
                }
                if let Some(attr) = group.attributes().get("media-source-default") {
                    media_source = attr.value().to_string();
                }
                if let Some(attr) = group.attributes().get("orientation-requested-default") {
                    let val_str = attr.value().to_string();
                    if val_str == "4" || val_str.to_lowercase().contains("landscape") {
                        orientation = "landscape".to_string();
                    } else {
                        orientation = "portrait".to_string();
                    }
                }
                if let Some(attr) = group.attributes().get("sides-default") {
                    sides = attr.value().to_string();
                }
                if let Some(attr) = group.attributes().get("print-color-mode-default") {
                    let val_str = attr.value().to_string();
                    if val_str.to_lowercase().contains("mono") || val_str.to_lowercase().contains("gray") {
                        color_mode = ColorMode::Monochrome;
                    } else {
                        color_mode = ColorMode::Color;
                    }
                } else if let Some(attr) = group.attributes().get("color-supported") {
                    if let Some(is_color) = attr.value().as_boolean() {
                        color_mode = if *is_color { ColorMode::Color } else { ColorMode::Monochrome };
                    }
                }
            }
        }
    }

    let props = crate::types::PrinterProperties {
        media,
        media_source,
        orientation,
        print_quality,
        sides,
    };

    (props, color_mode)
}

pub async fn save_printer_properties_via_ipp(
    name: &str,
    props: &crate::types::PrinterProperties,
    color_mode: &ColorMode,
    creds: Option<(&str, &str)>,
) -> Result<(), String> {
    let color_val = match color_mode {
        ColorMode::Color => "color",
        ColorMode::Monochrome => "monochrome",
    };

    let orient_val = match props.orientation.as_str() {
        "landscape" => 4,
        _ => 3,
    };

    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    let uri: Uri = uri_str.parse().map_err(|e| format!("Invalid printer URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();

    let mut request = IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsAddModifyPrinter, Some(uri));

    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("media-default", IppValue::Keyword(props.media.clone())),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("media-source-default", IppValue::Keyword(props.media_source.clone())),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("orientation-requested-default", IppValue::Enum(orient_val)),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("sides-default", IppValue::Keyword(props.sides.clone())),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("print-color-mode-default", IppValue::Keyword(color_val.to_string())),
    );

    match client.send(request).await {
        Ok(response) => {
            if response.header().status_code().is_success() {
                log::info!("Successfully saved printer properties for {} via IPP HTTP to localhost:631", name);
                Ok(())
            } else {
                Err(format!("CUPS IPP status error: {:?}", response.header().status_code()))
            }
        }
        Err(e) => Err(format!("Failed IPP HTTP request to localhost:631: {}", e)),
    }
}

pub async fn add_appsocket_printer_via_ipp(
    name: &str,
    ip: &str,
    port: u16,
    color_mode: ColorMode,
    creds: Option<(&str, &str)>,
) -> Result<(), String> {
    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    let uri: Uri = uri_str.parse().map_err(|e| format!("Invalid printer URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();

    let device_uri = format!("socket://{}:{}", ip.trim(), port);
    let mut request = IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsAddModifyPrinter, Some(uri));

    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("device-uri", IppValue::Uri(device_uri)),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("printer-is-accepting-jobs", IppValue::Boolean(true)),
    );

    let color_val = match color_mode {
        ColorMode::Color => "color",
        ColorMode::Monochrome => "monochrome",
    };
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("print-color-mode-default", IppValue::Keyword(color_val.to_string())),
    );

    match client.send(request).await {
        Ok(response) => {
            if response.header().status_code().is_success() {
                log::info!("Successfully added AppSocket printer {} via IPP HTTP to localhost:631", name);
                Ok(())
            } else {
                Err(format!("CUPS IPP status: {:?}", response.header().status_code()))
            }
        }
        Err(e) => Err(format!("Failed IPP HTTP request to localhost:631: {}", e)),
    }
}
