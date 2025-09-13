use crate::{
    read_config,
    types::{ColorMode, PrintAttributes, Printer},
};
use futures::io::Cursor;
use ipp::prelude::*;
use reqwest;
use tokio_util::bytes::Bytes;

pub struct PrinterManager {
    printers: Vec<Printer>,
    color_counter: usize,
    monochrome_counter: usize,
}

impl PrinterManager {
    pub fn new(printers: Vec<Printer>) -> Self {
        Self {
            printers,
            color_counter: 0,
            monochrome_counter: 0,
        }
    }

    pub fn get_printer(&mut self, color_mode: &ColorMode) -> Option<&Printer> {
        let color_mode_printers: Vec<_> = self
            .printers
            .iter()
            .filter(|p| p.color_mode == *color_mode)
            .collect();

        if color_mode_printers.is_empty() {
            return None;
        }

        match color_mode {
            ColorMode::Color => {
                let printer = color_mode_printers[self.color_counter % color_mode_printers.len()];
                self.color_counter += 1;
                Some(printer)
            }
            ColorMode::Monochrome => {
                let printer =
                    color_mode_printers[self.monochrome_counter % color_mode_printers.len()];
                self.monochrome_counter += 1;
                Some(printer)
            }
        }
    }
}

pub async fn print_job(
    printer_uri: Uri,
    attributes: PrintAttributes,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = download_file(attributes.file.clone()).await?;
    let payload = IppPayload::new_async(file);

    let print_job = IppOperationBuilder::print_job(printer_uri.clone(), payload)
        .attributes(build_ipp_attributes(attributes))
        .build();

    AsyncIppClient::new(printer_uri).send(print_job).await?;

    Ok(())
}

async fn download_file(file_id: String) -> Result<Cursor<Bytes>, Box<dyn std::error::Error>> {
    let base_url = read_config()?.s3_base_url;
    let file_url = format!("{}{}", base_url, file_id);
    let response = reqwest::get(file_url).await?;
    let bytes = response.bytes().await?;
    Ok(Cursor::new(bytes))
}

fn build_ipp_attributes(attributes: PrintAttributes) -> Vec<IppAttribute> {
    [
        ("orientation-requested", attributes.orientation),
        ("print-color-mode", attributes.color.to_val().to_string()),
        ("copies", attributes.copies),
        ("media", attributes.paper_format),
        ("page-ranges", attributes.page_ranges),
        ("number-up", attributes.number_up),
        ("sides", attributes.sides),
        ("document-format", attributes.document_format),
        ("print-scaling", attributes.print_scaling),
    ]
    .into_iter()
    .map(|(name, value)| IppAttribute::new(name, value.parse().unwrap()))
    .collect()
}

pub async fn get_ipp_printers() -> Result<Vec<Printer>, Box<dyn std::error::Error>> {
    let client = AsyncIppClient::builder("http://localhost:631".parse()?).build();
    let operation = IppOperationBuilder::cups().get_printers();
    let result = client.send(operation).await?;

    let mut printers: Vec<Printer> = Vec::new();

    for group in result
        .attributes()
        .groups_of(DelimiterTag::PrinterAttributes)
    {
        let color_mode = group.attributes()["color-supported"]
            .value()
            .as_boolean()
            .map(|is_color| match is_color {
                true => ColorMode::Color,
                false => ColorMode::Monochrome,
            })
            .unwrap();

        let uri = group.attributes()["printer-uri-supported"]
            .value()
            .to_string();

        printers.push(Printer { uri, color_mode });
    }

    Ok(printers)
}
