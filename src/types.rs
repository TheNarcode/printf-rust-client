use serde::Deserialize;

#[derive(Debug, PartialEq, Clone, Deserialize)]
pub enum ColorMode {
    Color,
    Monochrome,
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
    pub copies: String, // 9999 max hota hai
    pub paper_format: String,
}
