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

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Printer {
    pub uri: String,
    pub color_mode: ColorMode,
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub redis_url: String,
    pub s3_base_url: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub printf_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub file_id: String,
    pub attributes: PrintAttributes,
    pub status: String,
    pub updated_at: String,
}
