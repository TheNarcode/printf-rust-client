use crate::{
    read_config,
    types::{ColorMode, PrintAttributes},
};
use futures::io::Cursor;
use ipp::prelude::*;
use reqwest;
use tokio_util::bytes::Bytes;

pub struct PrinterManager {
    cindex: usize,
    clen: usize,
    bwindex: usize,
    bwlen: usize,
}

impl PrinterManager {
    pub fn new(clen: usize, bwlen: usize) -> Self {
        Self {
            cindex: 0,
            clen,
            bwindex: 0,
            bwlen,
        }
    }

    pub fn get_next_printer(&mut self, printer_type: &ColorMode) -> usize {
        match printer_type {
            ColorMode::Color => {
                let current_index = self.cindex;
                self.cindex = (self.cindex + 1) % self.clen;
                current_index
            }
            ColorMode::Monochrome => {
                let current_index = self.bwindex;
                self.bwindex = (self.bwindex + 1) % self.bwlen;
                current_index
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
    let base_url = read_config("config.json")?.s3_base_url;
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
