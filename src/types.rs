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
    #[serde(default)]
    pub printed: Option<bool>,
    #[serde(default)]
    pub footer: Option<bool>,
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

    #[serde(default)]
    pub cups_username: Option<String>,
    #[serde(default)]
    pub cups_password: Option<String>,
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
    #[serde(default)]
    pub lease_id: Option<String>,
    #[serde(default)]
    pub ipp_job_id: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiFile {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub orientation: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub copies: Option<String>,
    #[serde(default)]
    pub paper_format: Option<String>,
    #[serde(default)]
    pub page_ranges: Option<String>,
    #[serde(default)]
    pub number_up: Option<String>,
    #[serde(default)]
    pub sides: Option<String>,
    #[serde(default)]
    pub print_scaling: Option<String>,
    #[serde(default)]
    pub document_format: Option<String>,
    #[serde(default)]
    pub printed: Option<bool>,
}

impl ApiFile {
    pub fn to_print_attributes(
        &self,
        order_id: &str,
        printer_name: Option<String>,
        footer: Option<bool>,
    ) -> PrintAttributes {
        let color_mode = match self.color.as_deref() {
            Some("color") | Some("Color") => ColorMode::Color,
            _ => ColorMode::Monochrome,
        };

        PrintAttributes {
            file_id: self.file_id.clone(),
            orientation: self.orientation.clone().unwrap_or_else(|| "portrait".to_string()),
            color: color_mode,
            copies: self.copies.clone().unwrap_or_else(|| "1".to_string()),
            paper_format: self.paper_format.clone().unwrap_or_else(|| "iso_a4_210x297mm".to_string()),
            page_ranges: self.page_ranges.clone().unwrap_or_default(),
            number_up: self.number_up.clone().unwrap_or_else(|| "1".to_string()),
            sides: self.sides.clone().unwrap_or_else(|| "one-sided".to_string()),
            document_format: self.document_format.clone().unwrap_or_else(|| "application/octet-stream".to_string()),
            print_scaling: self.print_scaling.clone().unwrap_or_else(|| "auto".to_string()),
            target_printer: printer_name,
            order: self.order.clone().or_else(|| Some(order_id.to_string())),
            printed: self.printed,
            footer,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiOrder {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub payment_request_id: Option<String>,
    #[serde(default)]
    pub paid: Option<bool>,
    #[serde(default)]
    pub status: Option<i32>,
    #[serde(default)]
    pub printer_name: Option<String>,
    #[serde(default)]
    pub footer: Option<bool>,
    #[serde(default)]
    pub created_at: Option<serde_json::Value>,
    #[serde(default)]
    pub files: Vec<ApiFile>,
}

impl ApiOrder {
    pub fn to_print_attributes_list(&self) -> Vec<PrintAttributes> {
        self.files
            .iter()
            .map(|f| f.to_print_attributes(&self.id, self.printer_name.clone(), self.footer))
            .collect()
    }
}