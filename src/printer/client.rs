use std::sync::Arc;

use ipp::prelude::*;

use crate::state::AppState;
use crate::types::{ColorMode, Printer, PrinterProperties};

pub use crate::ipp::print_job;

/// Returns CUPS basic auth credentials `(username, password)` if configured in state.
pub fn get_cups_creds(config: &crate::types::Config) -> Option<(&str, &str)> {
    match (&config.cups_username, &config.cups_password) {
        (Some(u), Some(p)) if !u.is_empty() => Some((u.as_str(), p.as_str())),
        _ => None,
    }
}

/// Helper: formats an IPP URI, including basic-auth credentials if available.
pub fn format_ipp_uri(path: &str, creds: Option<(&str, &str)>) -> String {
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

/// Normalises a printer identifier into a CUPS URI path (`/printers/<name>`).
fn resolve_printer_path(printer_name_or_path: &str) -> String {
    if printer_name_or_path.starts_with('/') {
        printer_name_or_path.to_string()
    } else {
        format!("/printers/{}", printer_name_or_path)
    }
}

// ─── IPP printer list ─────────────────────────────────────────────────────────

/// Fetches the live printer list from CUPS via `CUPS-Get-Printers`.
pub async fn get_printer_list(
    state: Arc<AppState>,
) -> Result<Vec<Printer>, Box<dyn std::error::Error + Send + Sync>> {
    let creds = get_cups_creds(&state.config);
    get_ipp_printers(creds).await
}

/// Low-level: queries CUPS for all configured printers.
pub async fn get_ipp_printers(
    creds: Option<(&str, &str)>,
) -> Result<Vec<Printer>, Box<dyn std::error::Error + Send + Sync>> {
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
            .unwrap_or_else(|| uri.split('/').last().unwrap_or("Printer").to_string());

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

        let paused = group
            .attributes()
            .get("printer-state")
            .and_then(|attr| attr.value().as_enum())
            .map(|state| *state == 5) // 5 = printer-stopped (paused)
            .unwrap_or(false);

        let properties = Some(PrinterProperties {
            media: media_default,
            media_source: media_source_default,
            orientation: orientation_default,
            print_quality: "normal".to_string(),
            sides: sides_default,
        });

        printers.push(Printer {
            uri,
            name,
            color_mode,
            paused,
            properties,
        });
    }

    Ok(printers)
}

/// Convenience function for pausing a printer by URI.
pub async fn pause_printer(
    printer_uri: String,
    state: Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let creds = get_cups_creds(&state.config);
    let path = printer_uri.split('/').last().unwrap_or(&printer_uri);
    let uri_str = format_ipp_uri(&format!("/printers/{}", path), creds);
    let uri: Uri = uri_str.parse()?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let request = IppRequestResponse::new(IppVersion::v2_0(), Operation::PausePrinter, Some(uri));
    client.send(request).await?;
    Ok(())
}

/// Convenience function for resuming (unpausing) a printer by URI.
pub async fn unpause_printer(
    printer_uri: String,
    state: Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let creds = get_cups_creds(&state.config);
    let path = printer_uri.split('/').last().unwrap_or(&printer_uri);
    let uri_str = format_ipp_uri(&format!("/printers/{}", path), creds);
    let uri: Uri = uri_str.parse()?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let request = IppRequestResponse::new(IppVersion::v2_0(), Operation::ResumePrinter, Some(uri));
    client.send(request).await?;
    Ok(())
}

// ─── Printer Properties Fetching & Saving ────────────────────────────────────

/// Queries live IPP attributes for a single printer.
pub async fn fetch_printer_properties(
    printer_name: &str,
    state: Arc<AppState>,
) -> (PrinterProperties, ColorMode) {
    let creds = get_cups_creds(&state.config);
    fetch_printer_properties_via_ipp(printer_name, creds).await
}

pub async fn fetch_printer_properties_via_ipp(
    name: &str,
    creds: Option<(&str, &str)>,
) -> (PrinterProperties, ColorMode) {
    let mut media = "iso_a4_210x297mm".to_string();
    let mut media_source = "auto".to_string();
    let mut orientation = "portrait".to_string();
    let print_quality = "normal".to_string();
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
            for group in response
                .attributes()
                .groups_of(DelimiterTag::PrinterAttributes)
            {
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
                    let val_str = attr.value().to_string().to_lowercase();
                    if val_str.contains("monochrome") || val_str.contains("process-monochrome") {
                        color_mode = ColorMode::Monochrome;
                    } else {
                        color_mode = ColorMode::Color;
                    }
                } else if let Some(attr) = group.attributes().get("color-supported") {
                    if let Some(is_color) = attr.value().as_boolean() {
                        color_mode = if *is_color {
                            ColorMode::Color
                        } else {
                            ColorMode::Monochrome
                        };
                    }
                }
            }
        }
    }

    (
        PrinterProperties {
            media,
            media_source,
            orientation,
            print_quality,
            sides,
        },
        color_mode,
    )
}

/// Persists user-edited defaults back to CUPS via `CUPS-Add-Modify-Printer`.
pub async fn save_printer_properties(
    printer_name: String,
    props: PrinterProperties,
    color_mode: ColorMode,
    state: Arc<AppState>,
) -> Result<(), String> {
    let creds = get_cups_creds(&state.config);
    save_printer_properties_via_ipp(&printer_name, &props, color_mode, creds).await
}

pub async fn save_printer_properties_via_ipp(
    name: &str,
    props: &PrinterProperties,
    color_mode: ColorMode,
    creds: Option<(&str, &str)>,
) -> Result<(), String> {
    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    let uri: Uri = uri_str
        .parse()
        .map_err(|e| format!("Invalid printer URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();

    let color_val = match color_mode {
        ColorMode::Color => "color",
        ColorMode::Monochrome => "monochrome",
    };

    let mut request =
        IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsAddModifyPrinter, Some(uri));

    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            "media-default",
            IppValue::Keyword(props.media.clone()),
        ),
    );

    if props.media_source != "auto" {
        use std::collections::BTreeMap;
        let media_col = IppValue::Collection(BTreeMap::from([(
            "media-source".to_string(),
            IppValue::Keyword(props.media_source.clone()),
        )]));
        request.attributes_mut().add(
            DelimiterTag::PrinterAttributes,
            IppAttribute::new("media-col-default", media_col),
        );
    }

    let orientation_enum = if props.orientation == "landscape" { 4 } else { 3 };
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            "orientation-requested-default",
            IppValue::Enum(orientation_enum),
        ),
    );

    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            "sides-default",
            IppValue::Keyword(props.sides.clone()),
        ),
    );

    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            "print-color-mode-default",
            IppValue::Keyword(color_val.to_string()),
        ),
    );

    match client.send(request).await {
        Ok(resp) if resp.header().status_code().is_success() => {
            log::info!("Saved printer properties for {} via CUPS IPP", name);
            Ok(())
        }
        Ok(resp) => Err(format!(
            "CUPS IPP status error for {}: {:?}",
            name,
            resp.header().status_code()
        )),
        Err(e) => Err(format!("IPP request to CUPS failed for {}: {}", name, e)),
    }
}

// ─── PPD Queries & Add Printer ────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CupsPpdModel {
    pub ppd_name: String,
    pub description: String,
}

/// Queries installed CUPS PPD models directly from CUPS server via `CUPS-Get-PPDs`.
pub async fn get_cups_ppds(
    manufacturer_filter: Option<String>,
    state: &Arc<AppState>,
) -> Vec<CupsPpdModel> {
    let creds = get_cups_creds(&state.config);
    let mut list = Vec::new();

    let uri_str = format_ipp_uri("", creds);
    if let Ok(uri) = uri_str.parse::<Uri>() {
        let client = AsyncIppClient::builder(uri.clone()).build();
        let mut request =
            IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsGetPPDs, Some(uri));

        if let Some(ref mfg) = manufacturer_filter {
            request.attributes_mut().add(
                DelimiterTag::OperationAttributes,
                IppAttribute::new("ppd-make", IppValue::TextWithoutLanguage(mfg.clone())),
            );
        }

        if let Ok(response) = client.send(request).await {
            for group in response
                .attributes()
                .groups_of(DelimiterTag::PrinterAttributes)
            {
                let name = group
                    .attributes()
                    .get("ppd-name")
                    .map(|a| a.value().to_string())
                    .unwrap_or_default();
                let desc = group
                    .attributes()
                    .get("ppd-make-and-model")
                    .map(|a| a.value().to_string())
                    .unwrap_or_else(|| name.clone());

                if !name.is_empty() {
                    list.push(CupsPpdModel {
                        ppd_name: name,
                        description: desc,
                    });
                }
            }
        }
    }

    // Fallback if IPP CUPS-Get-PPDs returned empty (e.g. lpinfo -m local check)
    if list.is_empty() {
        if let Ok(output) = std::process::Command::new("lpinfo").arg("-m").output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        let name = parts[0].trim().to_string();
                        let desc = parts[1].trim().to_string();

                        if let Some(ref mfg) = manufacturer_filter {
                            if desc.to_lowercase().contains(&mfg.to_lowercase())
                                || name.to_lowercase().contains(&mfg.to_lowercase())
                            {
                                list.push(CupsPpdModel {
                                    ppd_name: name,
                                    description: desc,
                                });
                            }
                        } else {
                            list.push(CupsPpdModel {
                                ppd_name: name,
                                description: desc,
                            });
                        }
                    }
                }
            }
        }
    }

    list.sort_by(|a, b| a.description.cmp(&b.description));
    list
}

/// Registers an AppSocket (JetDirect) printer in CUPS via `CUPS-Add-Modify-Printer`.
pub async fn add_appsocket_printer(
    name: String,
    ip: String,
    port: u16,
    color_mode: ColorMode,
    ppd_name: Option<String>,
    ppd_file_bytes: Option<Vec<u8>>,
    state: Arc<AppState>,
) -> Result<String, String> {
    let clean_name = name.trim().replace(' ', "_");
    if clean_name.is_empty() || ip.trim().is_empty() {
        return Err("Printer name and IP address are required".to_string());
    }

    let creds = get_cups_creds(&state.config);
    add_appsocket_printer_via_ipp(
        &clean_name,
        &ip,
        port,
        color_mode,
        ppd_name.as_deref(),
        ppd_file_bytes,
        creds,
    )
    .await?;

    log::info!("Registered AppSocket printer {} ({}:{})", clean_name, ip, port);
    Ok(format!("Printer '{}' added successfully", clean_name))
}

/// Low-level: creates an AppSocket printer queue in CUPS via IPP.
pub async fn add_appsocket_printer_via_ipp(
    name: &str,
    ip: &str,
    port: u16,
    color_mode: ColorMode,
    ppd_name: Option<&str>,
    ppd_file_bytes: Option<Vec<u8>>,
    creds: Option<(&str, &str)>,
) -> Result<(), String> {
    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    let uri: Uri = uri_str
        .parse()
        .map_err(|e| format!("Invalid printer URI: {}", e))?;
    let client = AsyncIppClient::builder(uri.clone()).build();

    let device_uri = format!("socket://{}:{}", ip.trim(), port);
    let color_val = match color_mode {
        ColorMode::Color => "color",
        ColorMode::Monochrome => "monochrome",
    };

    let mut request =
        IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsAddModifyPrinter, Some(uri));

    if let Some(bytes) = ppd_file_bytes {
        let payload = IppPayload::new_async(futures::io::Cursor::new(bytes));
        *request.payload_mut() = payload;
    }

    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("device-uri", IppValue::Uri(device_uri.clone())),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new("printer-is-accepting-jobs", IppValue::Boolean(true)),
    );
    request.attributes_mut().add(
        DelimiterTag::PrinterAttributes,
        IppAttribute::new(
            "print-color-mode-default",
            IppValue::Keyword(color_val.to_string()),
        ),
    );

    if let Some(p) = ppd_name {
        if !p.trim().is_empty() {
            request.attributes_mut().add(
                DelimiterTag::PrinterAttributes,
                IppAttribute::new(
                    "ppd-name",
                    IppValue::NameWithoutLanguage(p.trim().to_string()),
                ),
            );
        }
    }

    match client.send(request).await {
        Ok(resp) if resp.header().status_code().is_success() => {
            log::info!(
                "Registered AppSocket printer {} (device-uri: {}) in CUPS",
                name, device_uri
            );
            Ok(())
        }
        Ok(resp) => Err(format!(
            "CUPS returned error for printer {}: {:?}",
            name,
            resp.header().status_code()
        )),
        Err(e) => Err(format!("IPP request to CUPS failed: {}", e)),
    }
}

// ─── IPP Job Cancellation ─────────────────────────────────────────────────────

/// Cancels a CUPS job by job ID.
pub async fn cancel_ipp_job(
    printer_name_or_path: &str,
    job_id: i32,
    creds: Option<(&str, &str)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = resolve_printer_path(printer_name_or_path);
    let uri_str = format_ipp_uri(&path, creds);
    let uri: Uri = uri_str
        .parse()
        .map_err(|e| format!("Invalid URI '{}': {}", uri_str, e))?;

    let client = AsyncIppClient::builder(uri.clone()).build();
    let operation = IppOperationBuilder::cancel_job(uri, job_id).build();

    let response = client.send(operation).await?;
    let status = response.header().status_code();

    if status.is_success() {
        log::info!("Successfully canceled CUPS job {} on {}", job_id, path);
        Ok(())
    } else {
        Err(format!(
            "CUPS returned error status {:?} when canceling job {}",
            status, job_id
        )
        .into())
    }
}

/// Deletes a printer queue from CUPS via `CUPS-Delete-Printer`.
pub async fn delete_printer(
    printer_name_or_uri: String,
    state: Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let creds = get_cups_creds(&state.config);
    let name = printer_name_or_uri.split('/').last().unwrap_or(&printer_name_or_uri);
    let uri_str = format_ipp_uri(&format!("/printers/{}", name), creds);
    let uri: Uri = uri_str.parse()?;
    let client = AsyncIppClient::builder(uri.clone()).build();
    let request = IppRequestResponse::new(IppVersion::v2_0(), Operation::CupsDeletePrinter, Some(uri));
    let response = client.send(request).await?;
    let status = response.header().status_code();

    if status.is_success() {
        log::info!("Successfully deleted printer {} from CUPS", name);
        Ok(())
    } else {
        Err(format!("CUPS returned error status {:?} when deleting printer {}", status, name).into())
    }
}
