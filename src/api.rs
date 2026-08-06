use std::sync::Arc;
use crate::constants::BASE_URL;
use crate::state::AppState;
use crate::types::ApiOrder;

fn get_api_base_url(state: &Arc<AppState>) -> &str {
    if !state.config.base_url.is_empty() {
        &state.config.base_url
    } else {
        BASE_URL
    }
}

pub async fn download_file(
    file_id: &str,
    state: &Arc<AppState>,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
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
    Ok(bytes.to_vec())
}

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
            status != 3 && (status == 1 || status == 2)
        })
        .collect();

    Ok(filtered)
}

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