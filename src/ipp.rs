use crate::types::{ColorMode, PrintAttributes, Printer};
use ipp::prelude::*;
use reqwest;
use std::{fs::File, io::Cursor};

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
    let deserialized_config: Vec<Printer> = serde_json::from_reader(file).unwrap();
    deserialized_config
}

pub async fn print(printer: &Printer, attributes: PrintAttributes) {
    let uri: Uri = printer.uri.parse().unwrap();
    let url =
        "https://pub-badb76cefc404f02a1f1bfa48ad9d871.r2.dev/0282fb6a-74a7-476c-aca1-18ada44593d8";

    let response = reqwest::get(url).await.unwrap();
    let bytes = response.bytes().await.unwrap();

    let file = Cursor::new(bytes);
    let payload = IppPayload::new(file);
    let builder = IppOperationBuilder::print_job(uri.clone(), payload);
    let client = AsyncIppClient::new(uri);
    let response = client.send(builder.build()).await.unwrap();
    println!("IPP status code: {}", response.header().status_code());
}
