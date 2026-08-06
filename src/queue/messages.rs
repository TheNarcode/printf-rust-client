use base64::Engine;
use crate::types::{
    ApiOrder, CfAckRequest, CfLeaseId, CfQueueMessage, CfQueuePullRequest, CfQueuePullResponse,
    PrintAttributes,
};


fn parse_body_str(s: &str) -> Result<Vec<PrintAttributes>, String> {
    if let Ok(order) = serde_json::from_str::<ApiOrder>(s) {
        if !order.files.is_empty() {
            return Ok(order.to_print_attributes_list());
        }
    }
    if let Ok(orders) = serde_json::from_str::<Vec<ApiOrder>>(s) {
        let list: Vec<_> = orders
            .into_iter()
            .flat_map(|o| o.to_print_attributes_list())
            .collect();
        if !list.is_empty() {
            return Ok(list);
        }
    }
    if let Ok(list) = serde_json::from_str::<Vec<PrintAttributes>>(s) {
        return Ok(list);
    }
    if let Ok(single) = serde_json::from_str::<PrintAttributes>(s) {
        return Ok(vec![single]);
    }
    Err(format!("Could not interpret message body string as PrintAttributes: {}", s))
}

pub fn parse_message_body(body: &serde_json::Value) -> Result<Vec<PrintAttributes>, String> {
    match body {
        serde_json::Value::String(s) => {
            if let Ok(decoded_bytes) =
                base64::engine::general_purpose::STANDARD.decode(s.as_bytes())
            {
                if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                    if let Ok(list) = parse_body_str(&decoded_str) {
                        return Ok(list);
                    }
                }
            }
            parse_body_str(s)
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            if let Ok(order) = serde_json::from_value::<ApiOrder>(body.clone()) {
                if !order.files.is_empty() {
                    return Ok(order.to_print_attributes_list());
                }
            }
            if let Ok(orders) = serde_json::from_value::<Vec<ApiOrder>>(body.clone()) {
                let list: Vec<_> = orders
                    .into_iter()
                    .flat_map(|o| o.to_print_attributes_list())
                    .collect();
                if !list.is_empty() {
                    return Ok(list);
                }
            }
            if let Ok(list) = serde_json::from_value::<Vec<PrintAttributes>>(body.clone()) {
                return Ok(list);
            }
            if let Ok(single) = serde_json::from_value::<PrintAttributes>(body.clone()) {
                return Ok(vec![single]);
            }
            Err(format!(
                "Could not interpret message body JSON as PrintAttributes: {}",
                body
            ))
        }
        other => Err(format!("Unexpected body type in queue message: {}", other)),
    }
}

pub async fn pull_cf_queue_messages(
    http_client: &reqwest::Client,
    account_id: &str,
    queue_id: &str,
    token: &str,
) -> Result<Vec<CfQueueMessage>, String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/queues/{}/messages/pull",
        account_id, queue_id
    );
    let payload = CfQueuePullRequest {
        visibility_timeout_ms: 180_000,
        batch_size: 10,
    };

    let resp = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("CF Queue pull request failed: {}", e))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("CF Queue pull API returned an error: {}", body));
    }

    let pull_resp: CfQueuePullResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to deserialise CF Queue pull response: {}", e))?;

    if !pull_resp.success {
        return Err(format!(
            "CF Queue pull API reported failure: {:?}",
            pull_resp.errors
        ));
    }

    Ok(pull_resp.result.map(|r| r.messages).unwrap_or_default())
}

pub async fn ack_cf_queue_messages(
    http_client: &reqwest::Client,
    account_id: &str,
    queue_id: &str,
    token: &str,
    acks: Vec<CfLeaseId>,
    retries: Vec<CfLeaseId>,
) -> Result<(), String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/queues/{}/messages/ack",
        account_id, queue_id
    );
    let payload = CfAckRequest { acks, retries };

    let resp = http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("CF Queue ack request failed: {}", e))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("CF Queue ack API returned an error: {}", body));
    }

    Ok(())
}