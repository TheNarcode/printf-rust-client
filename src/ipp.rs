use crate::types::{ColorMode, PrintAttributes, Printer};
use ipp::prelude::*;
use reqwest;
use std::{
    env,
    fs::File,
    io::{Cursor, Read},
};

pub struct PrinterManager {
    printers: Vec<Printer>,
    color_counter: usize,
    bw_counter: usize,
}

impl PrinterManager {
    pub fn new(file_name: &str) -> Self {
        Self {
            printers: read_config(file_name),
            color_counter: 0,
            bw_counter: 0,
        }
    }

    pub fn get_next_printer(&mut self, printer_type: &ColorMode) -> Option<&Printer> {
        match printer_type {
            ColorMode::Color => {
                let color_printers: Vec<usize> = self
                    .printers
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.printer_type == ColorMode::Color)
                    .map(|(i, _)| i)
                    .collect();

                if !color_printers.is_empty() {
                    let printer = &self.printers[color_printers[self.color_counter]];
                    self.color_counter = (self.color_counter + 1) % color_printers.len();
                    Some(printer)
                } else {
                    None
                }
            }
            ColorMode::Monochrome => {
                let bw_printers: Vec<usize> = self
                    .printers
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.printer_type == ColorMode::Monochrome)
                    .map(|(i, _)| i)
                    .collect();

                if !bw_printers.is_empty() {
                    let printer = &self.printers[bw_printers[self.bw_counter]];
                    self.bw_counter = (self.bw_counter + 1) % bw_printers.len();
                    Some(printer)
                } else {
                    None
                }
            }
        }
    }
}

pub fn read_config(file_name: &str) -> Vec<Printer> {
    let file = File::open(file_name).unwrap();
    serde_json::from_reader(file).unwrap()
}

pub async fn print(printer: &Printer, attributes: PrintAttributes) {
    let printer_uri: Uri = printer.uri.parse().unwrap();
    let file = download_file(attributes.file.clone()).await;
    let payload = IppPayload::new(file);

    let print_job = IppOperationBuilder::print_job(printer_uri.clone(), payload)
        .attributes(build_ipp_attributes(attributes))
        .build();

    AsyncIppClient::new(printer_uri)
        .send(print_job)
        .await
        .unwrap();
}

async fn download_file(file_id: String) -> impl Read {
    let base_url = env::var("R2_PUB_URL").unwrap();
    let file_url = format!("{}{}", base_url, file_id);
    let response = reqwest::get(file_url).await.unwrap();
    let bytes = response.bytes().await.unwrap();
    Cursor::new(bytes)
}

fn build_ipp_attributes(attributes: PrintAttributes) -> Vec<IppAttribute> {
    [
        ("orientation-requested", attributes.orientation),
        ("print-color-mode", attributes.color.to_val().to_string()),
        ("copies", attributes.copies),
        ("media", attributes.paper_format),
        ("page-ranges", attributes.page_ranges),
        ("number-up", attributes.number_up),
    ]
    .into_iter()
    .map(|(name, value)| IppAttribute::new(name, value.parse().unwrap()))
    .collect()
}
