use serde::Deserialize;

#[derive(Debug, PartialEq, Clone, Deserialize)]
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
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttributes {
    pub file: String,
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
    pub event_url: String,
    pub s3_base_url: String,
    pub monochrome_printers: Vec<Printer>,
    pub color_printers: Vec<Printer>,
}
