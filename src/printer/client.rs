use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use futures::io::Cursor;
use ipp::prelude::*;
use crate::state::AppState;
use crate::types::{ColorMode, PrintAttributes, Printer, PrinterProperties};

pub fn get_cups_creds(config: &crate::types::Config) -> Option<(&str, &str)> {
    match (&config.cups_username, &config.cups_password) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Some((u.as_str(), p.as_str())),
        _ => None,
    }
}

pub fn format_ipp_uri(path: &str, creds: Option<(&str, &str)>) -> String {
    if let Some((u, p)) = creds {
        if !u.is_empty() && !p.is_empty() {
            return format!("http://{}:{}@localhost:631{}", u, p, path);
        }
    }
    format!("http://localhost:631{}", path)
}

fn resolve_printer_path(s: &str) -> String {
    if s.starts_with('/') { s.to_string() } else { format!("/printers/{}", s) }
}

pub async fn get_printer_list(
    state: Arc<AppState>,
) -> Result<Vec<Printer>, Box<dyn std::error::Error + Send + Sync>> {
    get_ipp_printers(get_cups_creds(&state.config)).await
}

pub async fn get_ipp_printers(
    creds: Option<(&str, &str)>,
) -> Result<Vec<Printer>, Box<dyn std::error::Error + Send + Sync>> {
    let uri: Uri = format_ipp_uri("", creds).parse()?;
    let result = AsyncIppClient::builder(uri).build()
        .send(IppOperationBuilder::cups().get_printers())
        .await?;

    let mut printers = Vec::new();
    for group in result.attributes().groups_of(DelimiterTag::PrinterAttributes) {
        let color_mode = group.attributes().get("color-supported")
            .and_then(|a| a.value().as_boolean())
            .map(|b| if *b { ColorMode::Color } else { ColorMode::Monochrome })
            .unwrap_or(ColorMode::Color);
        let uri = group.attributes().get("printer-uri-supported")
            .map(|a| a.value().to_string()).unwrap_or_else(|| "unknown".to_string());
        let name = group.attributes().get("printer-name")
            .map(|a| a.value().to_string())
            .unwrap_or_else(|| uri.split('/').last().unwrap_or("Printer").to_string());
        let media = group.attributes().get("media-default")
            .map(|a| a.value().to_string()).unwrap_or_else(|| "iso_a4_210x297mm".to_string());
        let media_source = group.attributes().get("media-source-default")
            .map(|a| a.value().to_string()).unwrap_or_else(|| "auto".to_string());
        let orientation = group.attributes().get("orientation-requested-default")
            .map(|a| a.value().to_string()).unwrap_or_else(|| "portrait".to_string());
        let sides = group.attributes().get("sides-default")
            .map(|a| a.value().to_string()).unwrap_or_else(|| "one-sided".to_string());
        let paused = group.attributes().get("printer-state")
            .and_then(|a| a.value().as_enum()).map(|s| *s == 5).unwrap_or(false);
        printers.push(Printer {
            uri, name, color_mode, paused,
            properties: Some(PrinterProperties {
                media, media_source, orientation,
                print_quality: "normal".to_string(), sides,
            }),
        });
    }
    Ok(printers)
}

pub async fn pause_printer(printer_uri: String, state: Arc<AppState>)
    -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let creds = get_cups_creds(&state.config);
    let path = printer_uri.split('/').last().unwrap_or(&printer_uri);
    let uri: Uri = format_ipp_uri(&format!("/printers/{}", path), creds).parse()?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    client.send(IppRequestResponse::new(IppVersion::v2_0(), Operation::PausePrinter, Some(uri))).await?;
    if let Some(ref mut manager) = *state.printer_manager.lock().await {
        manager.set_printer_paused(&printer_uri, true);
    }
    Ok(())
}

pub async fn unpause_printer(printer_uri: String, state: Arc<AppState>)
    -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let creds = get_cups_creds(&state.config);
    let path = printer_uri.split('/').last().unwrap_or(&printer_uri);
    let uri: Uri = format_ipp_uri(&format!("/printers/{}", path), creds).parse()?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    client.send(IppRequestResponse::new(IppVersion::v2_0(), Operation::ResumePrinter, Some(uri))).await?;
    if let Some(ref mut manager) = *state.printer_manager.lock().await {
        manager.set_printer_paused(&printer_uri, false);
    }
    Ok(())
}

pub async fn fetch_printer_properties(printer_name: &str, state: Arc<AppState>) -> (PrinterProperties, ColorMode) {
    fetch_printer_properties_via_ipp(printer_name, get_cups_creds(&state.config)).await
}

pub(crate) async fn fetch_printer_properties_via_ipp(name: &str, creds: Option<(&str, &str)>) -> (PrinterProperties, ColorMode) {
    let mut media = "iso_a4_210x297mm".to_string();
    let mut media_source = "auto".to_string();
    let mut orientation = "portrait".to_string();
    let mut sides = "one-sided".to_string();
    let mut color_mode = ColorMode::Color;

    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    if let Ok(uri) = uri_str.parse::<Uri>() {
        let op = IppOperationBuilder::get_printer_attributes(uri.clone())
            .attributes(&["media-default","media-source-default","orientation-requested-default",
                           "sides-default","print-color-mode-default","color-supported"])
            .build();
        if let Ok(resp) = AsyncIppClient::builder(uri).build().send(op).await {
            for group in resp.attributes().groups_of(DelimiterTag::PrinterAttributes) {
                if let Some(a) = group.attributes().get("media-default") { media = a.value().to_string(); }
                if let Some(a) = group.attributes().get("media-source-default") { media_source = a.value().to_string(); }
                if let Some(a) = group.attributes().get("orientation-requested-default") {
                    let v = a.value().to_string();
                    orientation = if v == "4" || v.to_lowercase().contains("landscape") { "landscape".to_string() } else { "portrait".to_string() };
                }
                if let Some(a) = group.attributes().get("sides-default") { sides = a.value().to_string(); }
                if let Some(a) = group.attributes().get("print-color-mode-default") {
                    let v = a.value().to_string().to_lowercase();
                    color_mode = if v.contains("monochrome") || v.contains("process-monochrome") { ColorMode::Monochrome } else { ColorMode::Color };
                } else if let Some(a) = group.attributes().get("color-supported") {
                    if let Some(b) = a.value().as_boolean() {
                        color_mode = if *b { ColorMode::Color } else { ColorMode::Monochrome };
                    }
                }
            }
        }
    }
    (PrinterProperties { media, media_source, orientation, print_quality: "normal".to_string(), sides }, color_mode)
}

pub async fn save_printer_properties(printer_name: String, props: PrinterProperties, color_mode: ColorMode, state: Arc<AppState>) -> Result<(), String> {
    save_printer_properties_via_ipp(&printer_name, &props, color_mode, get_cups_creds(&state.config)).await
}

pub(crate) async fn save_printer_properties_via_ipp(name: &str, props: &PrinterProperties, color_mode: ColorMode, creds: Option<(&str, &str)>) -> Result<(), String> {
    let uri: Uri = format_ipp_uri(&format!("/printers/{}", name), creds)
        .parse().map_err(|e| format!("Invalid printer URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let color_val = match color_mode { ColorMode::Color => "color", ColorMode::Monochrome => "monochrome" };
    let orient_enum = if props.orientation == "landscape" { 4i32 } else { 3i32 };

    let mut req = IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsAddModifyPrinter, Some(uri));
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("media-default", IppValue::Keyword(props.media.clone())));
    if props.media_source != "auto" {
        let mc = IppValue::Collection(BTreeMap::from([("media-source".to_string(), IppValue::Keyword(props.media_source.clone()))]));
        req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("media-col-default", mc));
    }
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("orientation-requested-default", IppValue::Enum(orient_enum)));
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("sides-default", IppValue::Keyword(props.sides.clone())));
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("print-color-mode-default", IppValue::Keyword(color_val.to_string())));

    match client.send(req).await {
        Ok(r) if r.header().status_code().is_success() => { log::info!("Saved printer properties for {}", name); Ok(()) }
        Ok(r) => Err(format!("CUPS IPP error for {}: {:?}", name, r.header().status_code())),
        Err(e) => Err(format!("IPP request failed for {}: {}", name, e)),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CupsPpdModel { pub ppd_name: String, pub description: String }

pub async fn get_cups_ppds(manufacturer_filter: Option<String>, state: &Arc<AppState>) -> Vec<CupsPpdModel> {
    let creds = get_cups_creds(&state.config);
    let mut list = Vec::new();
    let uri_str = format_ipp_uri("", creds);
    if let Ok(uri) = uri_str.parse::<Uri>() {
        let client = AsyncIppClient::builder(uri.clone()).build();
        let mut req = IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsGetPPDs, Some(uri));
        if let Some(ref mfg) = manufacturer_filter {
            req.attributes_mut().add(DelimiterTag::OperationAttributes,
                IppAttribute::new("ppd-make", IppValue::TextWithoutLanguage(mfg.clone())));
        }
        if let Ok(resp) = client.send(req).await {
            for group in resp.attributes().groups_of(DelimiterTag::PrinterAttributes) {
                let name = group.attributes().get("ppd-name").map(|a| a.value().to_string()).unwrap_or_default();
                let desc = group.attributes().get("ppd-make-and-model").map(|a| a.value().to_string()).unwrap_or_else(|| name.clone());
                if !name.is_empty() { list.push(CupsPpdModel { ppd_name: name, description: desc }); }
            }
        }
    }
    if list.is_empty() {
        if let Ok(out) = std::process::Command::new("lpinfo").arg("-m").output() {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        let name = parts[0].trim().to_string();
                        let desc = parts[1].trim().to_string();
                        if let Some(ref mfg) = manufacturer_filter {
                            if !desc.to_lowercase().contains(&mfg.to_lowercase()) && !name.to_lowercase().contains(&mfg.to_lowercase()) { continue; }
                        }
                        list.push(CupsPpdModel { ppd_name: name, description: desc });
                    }
                }
            }
        }
    }
    list.sort_by(|a, b| a.description.cmp(&b.description));
    list
}

pub async fn add_appsocket_printer(name: String, ip: String, port: u16, color_mode: ColorMode, ppd_name: Option<String>, ppd_file_bytes: Option<Vec<u8>>, state: Arc<AppState>) -> Result<String, String> {
    let clean = name.trim().replace(' ', "_");
    if clean.is_empty() || ip.trim().is_empty() { return Err("Printer name and IP address are required".to_string()); }
    add_appsocket_printer_via_ipp(&clean, &ip, port, color_mode, ppd_name.as_deref(), ppd_file_bytes, get_cups_creds(&state.config)).await?;
    log::info!("Registered AppSocket printer {} ({}:{})", clean, ip, port);
    Ok(format!("Printer '{}' added successfully", clean))
}

pub(crate) async fn add_appsocket_printer_via_ipp(name: &str, ip: &str, port: u16, color_mode: ColorMode, ppd_name: Option<&str>, ppd_file_bytes: Option<Vec<u8>>, creds: Option<(&str, &str)>) -> Result<(), String> {
    let uri: Uri = format_ipp_uri(&format!("/printers/{}", name), creds).parse().map_err(|e| format!("Invalid URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let device_uri = format!("socket://{}:{}", ip.trim(), port);
    let color_val = match color_mode { ColorMode::Color => "color", ColorMode::Monochrome => "monochrome" };
    let mut req = IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsAddModifyPrinter, Some(uri));
    if let Some(bytes) = ppd_file_bytes {
        *req.payload_mut() = IppPayload::new_async(futures::io::Cursor::new(bytes));
    }
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("device-uri", IppValue::Uri(device_uri.clone())));
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("printer-is-accepting-jobs", IppValue::Boolean(true)));
    req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("print-color-mode-default", IppValue::Keyword(color_val.to_string())));
    if let Some(p) = ppd_name {
        if !p.trim().is_empty() {
            req.attributes_mut().add(DelimiterTag::PrinterAttributes, IppAttribute::new("ppd-name", IppValue::NameWithoutLanguage(p.trim().to_string())));
        }
    }
    match client.send(req).await {
        Ok(r) if r.header().status_code().is_success() => { log::info!("Added AppSocket printer {} ({})", name, device_uri); Ok(()) }
        Ok(r) => Err(format!("CUPS error for {}: {:?}", name, r.header().status_code())),
        Err(e) => Err(format!("IPP request to CUPS failed: {}", e)),
    }
}

pub async fn cancel_ipp_job(printer_name_or_path: &str, job_id: i32, creds: Option<(&str, &str)>)
    -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let path = resolve_printer_path(printer_name_or_path);
    let uri: Uri = format_ipp_uri(&path, creds).parse().map_err(|e| format!("Invalid URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let resp = client.send(IppOperationBuilder::cancel_job(uri, job_id).build()).await?;
    if resp.header().status_code().is_success() {
        log::info!("Cancelled CUPS job {} on {}", job_id, path);
        Ok(())
    } else {
        Err(format!("CUPS error {:?} canceling job {}", resp.header().status_code(), job_id).into())
    }
}

pub async fn delete_printer(printer_name_or_uri: String, state: Arc<AppState>)
    -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let creds = get_cups_creds(&state.config);
    let name = printer_name_or_uri.split('/').last().unwrap_or(&printer_name_or_uri);
    let uri: Uri = format_ipp_uri(&format!("/printers/{}", name), creds).parse()?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let resp = client.send(IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsDeletePrinter, Some(uri))).await?;
    if resp.header().status_code().is_success() {
        log::info!("Deleted printer {} from CUPS", name); Ok(())
    } else {
        Err(format!("CUPS error {:?} deleting printer {}", resp.header().status_code(), name).into())
    }
}

fn build_ipp_attributes(attributes: PrintAttributes, media_source: Option<String>) -> Vec<IppAttribute> {
    let mut copies_count = 1i32;
    let mut attrs: Vec<IppAttribute> = [
        ("orientation-requested", attributes.orientation.clone()),
        ("print-color-mode", attributes.color.to_val().to_string()),
        ("copies", attributes.copies.clone()),
        ("media", attributes.paper_format.clone()),
        ("page-ranges", attributes.page_ranges.clone()),
        ("number-up", attributes.number_up.clone()),
        ("sides", attributes.sides.clone()),
        ("print-scaling", attributes.print_scaling.clone()),
        ("document-format", "application/octet-stream".to_string()),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .filter_map(|(name, value)| match name {
        "copies" => match value.parse::<i32>() {
            Ok(v) => { copies_count = v; Some(IppAttribute::new(name, IppValue::Integer(v))) }
            Err(e) => { log::warn!("Skipping '{}': {}", name, e); None }
        },
        "number-up" => match value.parse::<i32>() {
            Ok(v) => Some(IppAttribute::new(name, IppValue::Integer(v))),
            Err(e) => { log::warn!("Skipping '{}': {}", name, e); None }
        },
        "orientation-requested" => match value.parse::<i32>() {
            Ok(v) => Some(IppAttribute::new(name, IppValue::Enum(v))),
            Err(e) => { log::warn!("Skipping '{}': {}", name, e); None }
        },
        _ => match value.parse() {
            Ok(v) => Some(IppAttribute::new(name, v)),
            Err(e) => { log::warn!("Skipping '{}': {}", name, e); None }
        },
    })
    .collect();

    if copies_count > 1 {
        attrs.push(IppAttribute::new("multiple-document-handling", IppValue::Keyword("separate-documents-uncollated-copies".to_string())));
    }
    if let Some(src) = media_source {
        attrs.push(IppAttribute::new("media-col", IppValue::Collection(BTreeMap::from([("media-source".to_string(), IppValue::Keyword(src))]))));
    }
    match attributes.sides.as_str() {
        "two-sided-short-edge" => {
            attrs.push(IppAttribute::new("BindEdge", IppValue::Keyword("Top".to_string())));
            attrs.push(IppAttribute::new("binding-edge", IppValue::Keyword("top".to_string())));
            attrs.push(IppAttribute::new("Binding", IppValue::Keyword("TopBinding".to_string())));
            attrs.push(IppAttribute::new("KMDuplex", IppValue::Keyword("2Sided".to_string())));
        }
        "two-sided-long-edge" => {
            attrs.push(IppAttribute::new("BindEdge", IppValue::Keyword("Left".to_string())));
            attrs.push(IppAttribute::new("binding-edge", IppValue::Keyword("left".to_string())));
            attrs.push(IppAttribute::new("Binding", IppValue::Keyword("LeftBinding".to_string())));
            attrs.push(IppAttribute::new("KMDuplex", IppValue::Keyword("2Sided".to_string())));
        }
        _ => {}
    }
    attrs
}

pub async fn print_job(
    printer_uri: Uri,
    printer_name: String,
    mut attributes: PrintAttributes,
    media_source: Option<String>,
    state: Arc<AppState>
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let raw_bytes = crate::api::download_file
    let sliced = if !attributes.page_ranges.trim().is_empty() {
        match crate::printer::pdf::slice_pdf_bytes(&raw_bytes, &attributes.page_ranges) {
            Ok(s) => { attributes.page_ranges = String::new(); s }
            Err(e) => { log::warn!("PDF slicing failed ({}); using original", e); raw_bytes }
        }
    } else { raw_bytes };

    let final_bytes = match crate::printer::pdf::process_pdf_footer(&sliced, &attributes) {
        Ok(b) => b,
        Err(e) => { log::warn!("PDF footer processing failed ({}); using sliced bytes", e); sliced }
    };

    let payload = IppPayload::new_async(Cursor::new(final_bytes));
    let op = IppOperationBuilder::print_job(printer_uri.clone(), payload)
        .attributes(build_ipp_attributes(attributes.clone(), media_source))
        .build();
    let resp = AsyncIppClient::new(printer_uri.clone()).send(op).await?;
    let job_id = resp.attributes()
        .groups_of(DelimiterTag::JobAttributes).next()
        .and_then(|g| g.attributes().get("job-id"))
        .and_then(|a| a.value().as_integer()).copied()
        .ok_or("No job-id in CUPS print response")?;

    log::info!("Job {} accepted by CUPS (IPP #{}) — polling...", attributes.file_id, job_id);
    { let mut s = state.job_store.lock().await; if let Some(i) = s.get_mut(&attributes.file_id) { i.ipp_job_id = Some(job_id); } }

    let ipp_client = AsyncIppClient::new(printer_uri.clone());
    let mut pending_secs = 0u32;
    let mut proc_stopped_secs = 0u32;
    let mut processing_secs = 0u32;

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let get_op = IppOperationBuilder::get_job_attributes(printer_uri.clone(), job_id).build();

        match ipp_client.send(get_op).await {
            Ok(r) => {
                if r.header().status_code() == ipp::model::StatusCode::ClientErrorNotFound {
                    log::info!("Job #{} purged by CUPS — completed", job_id);
                    let _ = crate::api::notify_webhook(&attributes.file_id, &printer_name, &state).await;
                    return Ok(());
                }

                let mut job_state: Option<i32> = None;
                let mut reasons: Vec<String> = Vec::new();
                let mut message: Option<String> = None;

                if let Some(group) = r.attributes().groups_of(DelimiterTag::JobAttributes).next() {
                    if let Some(a) = group.attributes().get("job-state") {
                        if let Some(&s) = a.value().as_enum() { job_state = Some(s); }
                    }
                    if let Some(a) = group.attributes().get("job-state-reasons") {
                        match a.value() {
                            IppValue::Keyword(k) => reasons.push(k.clone()),
                            IppValue::Array(arr) => { for item in arr { reasons.push(item.to_string()); } }
                            v => reasons.push(v.to_string()),
                        }
                    }
                    if let Some(a) = group.attributes().get("job-state-message") { message = Some(a.value().to_string()); }
                }

                let Some(state_val) = job_state else {
                    log::info!("Job #{} has no state — CUPS purged it, completed", job_id);
                    let _ = crate::api::notify_webhook(&attributes.file_id, &printer_name, &state).await;
                    return Ok(());
                };

                let reasons_str = reasons.join(", ");
                log::debug!("Job #{} state={} reasons=[{}]", job_id, state_val, reasons_str);

                if state_val == 9 {
                    log::info!("Job #{} completed", job_id);
                    let _ = crate::api::notify_webhook(&attributes.file_id, &printer_name, &state).await;
                    return Ok(());
                }
                if state_val == 7 || state_val == 8 {
                    let d = message.or_else(|| if !reasons.is_empty() { Some(reasons_str) } else { None })
                        .unwrap_or_else(|| format!("state {}", state_val));
                    return Err(format!("PendingTimeout: Job aborted/canceled ({})", d).into());
                }

                let has_error = reasons.iter().any(|r| {
                    let l = r.to_lowercase();
                    l.contains("error") || l.contains("aborted") || l.contains("canceled")
                    || l.contains("offline") || l.contains("media-jam") || l.contains("toner-empty")
                    || l.contains("door-open") || l.contains("tray-missing")
                });
                if has_error {
                    let d = message.or_else(|| if !reasons.is_empty() { Some(reasons_str) } else { None })
                        .unwrap_or_else(|| format!("state {}", state_val));
                    let _ = ipp_client.send(IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build()).await;
                    return Err(format!("PendingTimeout: Printer error ({})", d).into());
                }

                match state_val {
                    3 | 4 => {
                        pending_secs += 1; proc_stopped_secs = 0; processing_secs = 0;
                        if pending_secs >= 30 {
                            let _ = ipp_client.send(IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build()).await;
                            return Err(format!("PendingTimeout: Job #{} stuck pending {}s", job_id, pending_secs).into());
                        }
                    }
                    5 => {
                        processing_secs += 1; pending_secs = 0; proc_stopped_secs = 0;
                        if processing_secs >= 120 {
                            let _ = ipp_client.send(IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build()).await;
                            return Err(format!("PendingTimeout: Job #{} processing {}s", job_id, processing_secs).into());
                        }
                    }
                    6 => {
                        proc_stopped_secs += 1; pending_secs = 0; processing_secs = 0;
                        if proc_stopped_secs >= 15 {
                            let _ = ipp_client.send(IppOperationBuilder::cancel_job(printer_uri.clone(), job_id).build()).await;
                            return Err(format!("PendingTimeout: Job #{} proc-stopped {}s", job_id, proc_stopped_secs).into());
                        }
                    }
                    _ => { pending_secs = 0; proc_stopped_secs = 0; processing_secs = 0; }
                }
            }
            Err(e) => {
                let es = e.to_string();
                if es.to_lowercase().contains("not-found") || es.contains("404") {
                    log::info!("Job #{} not-found on poll — completed", job_id);
                    let _ = crate::api::notify_webhook(&attributes.file_id, &printer_name, &state).await;
                    return Ok(());
                }
                return Err(format!("Error polling job #{}: {}", job_id, e).into());
            }
        }
    }
}