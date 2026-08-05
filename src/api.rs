use std::sync::Arc;

use futures::io::Cursor;
use tokio_util::bytes::Bytes;

use crate::constants::BASE_URL;
use crate::state::AppState;
use crate::types::ApiOrder;

/// Helper to get the active base URL from state config, falling back to constant.
fn get_api_base_url(state: &Arc<AppState>) -> &str {
    if !state.config.base_url.is_empty() {
        &state.config.base_url
    } else {
        BASE_URL
    }
}

// ─── File download ────────────────────────────────────────────────────────────

/// Downloads a print file from the configured S3-compatible object store.
///
/// Returns a `Cursor<Bytes>` ready to be streamed into an IPP payload.
/// The `reqwest::Client` already has a timeout configured at construction time,
/// so no per-request timeout is needed here.
pub async fn download_file(
    file_id: &str,
    state: &Arc<AppState>,
) -> Result<Cursor<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}{}", state.config.s3_base_url, file_id);
    log::info!("Downloading file {} from {}", file_id, url);

    let response = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("HTTP GET failed for file {}: {}", file_id, e))?;

    if !response.status().is_success() {
        return Err(format!(
            "File download for {} failed — server returned HTTP {}",
            file_id,
            response.status()
        )
        .into());
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response body for file {}: {}", file_id, e))?;

    log::info!("Downloaded {} bytes for file {}", bytes.len(), file_id);
    Ok(Cursor::new(bytes))
}

// ─── Webhook notification ──────────────────────────────────────────────────────

/// Fires the configured webhook URL (or defaults to `{base_url}/webhook/notify`)
/// when a print job completes successfully.
///
/// Side effect on printfs server:
/// 1. Marks file as printed (`printed: true`) in database.
/// 2. If all files in the order are printed, sets order status = 1 (printed) and
///    triggers FCM push notification ("Order#... completed") to customer's device.
pub async fn notify_webhook(
    file_id: &str,
    printer_name: &str,
    state: &Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let base_url = get_api_base_url(state);
    let webhook_url = match &state.config.webhook_url {
        Some(url) if !url.is_empty() => url.clone(),
        _ => format!("{}/webhook/notify", base_url),
    };

    let payload = serde_json::json!({
        "id":          file_id,
        "printerName": printer_name,
    });

    log::info!("Sending print completion notification for file {} to {}", file_id, webhook_url);

    let mut req = state.http_client.post(&webhook_url).json(&payload);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("X-Printf-Key", key.as_str());
    }

    let response = req
        .send()
        .await
        .map_err(|e| format!("Webhook POST to {} failed: {}", webhook_url, e))?;

    let status = response.status();
    if !status.is_success() {
        let err_body = response.text().await.unwrap_or_default();
        return Err(format!("Webhook returned HTTP {} — body: {}", status, err_body).into());
    }

    log::info!("Webhook notified successfully for file {}", file_id);
    Ok(())
}

// ─── printfs API ──────────────────────────────────────────────────────────────

/// Fetches print statistics for the given time window.
///
/// `month` must be one of the values the API accepts:
///   - `"current"` — current calendar month
///   - `"past"`    — previous calendar month
///   - `"three"`   — rolling last 3 months
///   - `"all"` / `None` — all time (no filter applied)
///
/// The Cloudflare Worker validates the `month` query param with Zod's
/// `z.enum(["current","past","three","all"])`. Any other value is rejected (400).
pub async fn get_stats(
    month: Option<String>,
    state: Arc<AppState>,
) -> Result<serde_json::Value, String> {
    let base_url = get_api_base_url(&state);
    let url = match month.as_deref() {
        Some(m) if !m.is_empty() && m != "all" => {
            format!("{}/client/stats?month={}", base_url, m)
        }
        _ => format!("{}/client/stats", base_url),
    };

    let mut req = state.http_client.get(&url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("X-Printf-Key", key.as_str());
    }

    req.send()
        .await
        .map_err(|e| format!("Failed to fetch stats: {}", e))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse stats response: {}", e))
}

/// Fetches all orders that are paid/printed but have NOT yet been physically
/// collected by the customer (status != 3).
///
/// This is intentionally a "Ready for Pickup" view — once an order is marked
/// collected it disappears from the list.
pub async fn get_completed_orders(state: Arc<AppState>) -> Result<Vec<ApiOrder>, String> {
    let base_url = get_api_base_url(&state);
    let url = format!("{}/client/orders", base_url);
    let mut req = state.http_client.get(&url);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("X-Printf-Key", key.as_str());
    }

    let orders = req
        .send()
        .await
        .map_err(|e| format!("Failed to fetch orders: {}", e))?
        .json::<Vec<ApiOrder>>()
        .await
        .map_err(|e| format!("Failed to parse orders response: {}", e))?;

    let filtered = orders
        .into_iter()
        .filter(|o| {
            let status = o.status.unwrap_or(0);
            // An order is ready for pickup ONLY when status is printed/completed (status 1 or 2) and not yet collected (status 3)
            status != 3 && (status == 1 || status == 2)
        })
        .collect();

    Ok(filtered)
}

/// Marks an order as collected (`status = 3`) on the printfs API.
///
/// Side effect: the API fires an FCM push notification ("Order#... collected")
/// to the customer's device.
pub async fn mark_order_collected(order_id: String, state: Arc<AppState>) -> Result<(), String> {
    let base_url = get_api_base_url(&state);
    let url = format!("{}/client/collect", base_url);
    let payload = serde_json::json!({ "orderId": order_id });

    log::info!("Calling /client/collect for order {}", order_id);

    let mut req = state.http_client.post(&url).json(&payload);
    if let Some(ref key) = state.config.printf_key {
        req = req.header("X-Printf-Key", key.as_str());
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Failed to send collect request for order {}: {}", order_id, e))?;

    let status = resp.status();
    if status.is_success() {
        log::info!("Order {} marked as collected successfully via API", order_id);
        Ok(())
    } else {
        let err_text = resp.text().await.unwrap_or_default();
        log::error!("collect API returned HTTP {} for order {}: {}", status, order_id, err_text);
        Err(format!("collect API returned HTTP {} for order {}: {}", status, order_id, err_text))
    }
}
