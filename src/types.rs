use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub enum ColorMode {
    Color,
    Monochrome,
}

impl ColorMode {
    pub fn to_val(&self) -> &str {
        match self {
            ColorMode::Color => "color",
            ColorMode::Monochrome => "monochrome",
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterProperties {
    #[serde(default = "default_media")]
    pub media: String,
    #[serde(default = "default_media_source")]
    pub media_source: String,
    #[serde(default = "default_orientation")]
    pub orientation: String,
    #[serde(default = "default_print_quality")]
    pub print_quality: String,
    #[serde(default = "default_sides")]
    pub sides: String,
}

fn default_media() -> String { "iso_a4_210x297mm".to_string() }
fn default_media_source() -> String { "auto".to_string() }
fn default_orientation() -> String { "portrait".to_string() }
fn default_print_quality() -> String { "normal".to_string() }
fn default_sides() -> String { "one-sided".to_string() }

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Printer {
    pub uri: String,
    pub name: String,
    pub color_mode: ColorMode,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub properties: Option<PrinterProperties>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttributes {
    pub file_id: String,
    pub orientation: String,
    pub color: ColorMode,
    pub copies: String,
    pub paper_format: String,
    pub page_ranges: String,
    pub number_up: String,
    pub sides: String,
    pub document_format: String,
    pub print_scaling: String,
    #[serde(default)]
    pub target_printer: Option<String>,
    #[serde(alias = "orderId", default)]
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub s3_base_url: String,

    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub printf_key: Option<String>,
    pub base_url: String,
    #[serde(default, alias = "cf_account_id", alias = "accountId")]
    pub cf_account_id: Option<String>,
    #[serde(default, alias = "cf_queue_id", alias = "queueId", alias = "cf_queue_name", alias = "queueName")]
    pub cf_queue_id: Option<String>,
    #[serde(default, alias = "cf_api_token", alias = "apiToken", alias = "cf_token")]
    pub cf_api_token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CfQueuePullRequest {
    pub visibility_timeout_ms: u32,
    pub batch_size: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CfQueueMessage {
    pub id: String,
    pub lease_id: String,
    pub body: serde_json::Value,
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
    #[serde(default)]
    pub attempts: Option<u32>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CfQueuePullResult {
    #[serde(default)]
    pub message_backlog_count: Option<u64>,
    #[serde(default)]
    pub messages: Vec<CfQueueMessage>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CfQueuePullResponse {
    pub success: bool,
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
    #[serde(default)]
    pub result: Option<CfQueuePullResult>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CfLeaseId {
    pub lease_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay_seconds: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CfAckRequest {
    pub acks: Vec<CfLeaseId>,
    pub retries: Vec<CfLeaseId>,
}


#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub file_id: String,
    pub order_id: Option<String>,
    pub attributes: PrintAttributes,
    pub status: String,
    pub updated_at: String,
}

// Types for the /client/orders API response
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiFile {
    pub file_id: String,
    pub order: String,
    pub orientation: String,
    pub color: String,
    pub copies: String,
    pub paper_format: String,
    pub page_ranges: String,
    pub number_up: String,
    pub sides: String,
    pub print_scaling: String,
    pub document_format: String,
    pub printed: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiOrder {
    pub email: String,
    pub id: String,
    pub amount: f64,
    pub payment_request_id: String,
    pub paid: bool,
    pub status: i32,
    pub printer_name: Option<String>,
    pub created_at: String,
    pub files: Vec<ApiFile>,
}
