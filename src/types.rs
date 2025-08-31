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
    pub printer_type: ColorMode,
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
}
