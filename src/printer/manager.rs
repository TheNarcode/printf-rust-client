use crate::types::{ColorMode, Printer, PrinterProperties};

/// Manages the in-process set of known printers and distributes jobs using
/// round-robin load balancing, independently per color mode.
pub struct PrinterManager {
    printers: Vec<Printer>,
    color_counter: usize,
    monochrome_counter: usize,
    order_counter: usize,
}

impl PrinterManager {
    pub fn new(printers: Vec<Printer>) -> Self {
        Self {
            printers,
            color_counter: 0,
            monochrome_counter: 0,
            order_counter: 0,
        }
    }

    pub fn get_printers(&self) -> Vec<Printer> {
        self.printers.clone()
    }

    /// Marks a printer as paused or active. Paused printers are excluded from
    /// future `get_printers_for_order` selections.
    pub fn set_printer_paused(&mut self, uri: &str, paused: bool) {
        if let Some(p) = self.printers.iter_mut().find(|p| p.uri == uri) {
            p.paused = paused;
            log::info!("Printer {} paused={}", uri, paused);
        } else {
            log::warn!("set_printer_paused: no printer found with URI {}", uri);
        }
    }

    /// Updates a printer's properties and color mode in the in-memory list.
    pub fn set_printer_properties(
        &mut self,
        name: &str,
        properties: PrinterProperties,
        color_mode: ColorMode,
    ) {
        if let Some(p) = self
            .printers
            .iter_mut()
            .find(|p| p.name == name || p.uri.contains(name))
        {
            p.properties = Some(properties);
            p.color_mode = color_mode;
        }
    }

    /// Selects one color printer and one monochrome printer for an order using
    /// independent round-robin counters.
    ///
    /// Returns `(color_printer, mono_printer, color_media_source, mono_media_source)`.
    ///
    /// **Bug 3 fix**: `media_source` is now extracted from each selected printer's
    /// configured `properties.media_source`. Previously both were always `None`,
    /// making the "Default Input Tray" setting a no-op.
    pub fn get_printers_for_order(
        &mut self,
        has_color: bool,
        has_mono: bool,
    ) -> (Option<Printer>, Option<Printer>, Option<String>, Option<String>) {
        self.order_counter += 1;

        let color_printer = if has_color {
            let available: Vec<_> = self
                .printers
                .iter()
                .filter(|p| p.color_mode == ColorMode::Color && !p.paused)
                .collect();
            if !available.is_empty() {
                let p = available[self.color_counter % available.len()].clone();
                self.color_counter += 1;
                Some(p)
            } else {
                None
            }
        } else {
            None
        };

        let mono_printer = if has_mono {
            let available: Vec<_> = self
                .printers
                .iter()
                .filter(|p| p.color_mode == ColorMode::Monochrome && !p.paused)
                .collect();
            if !available.is_empty() {
                let p = available[self.monochrome_counter % available.len()].clone();
                self.monochrome_counter += 1;
                Some(p)
            } else {
                None
            }
        } else {
            None
        };

        // Extract the configured `media-source` from each selected printer's properties
        // so that `dispatch_job_batch` can pass it to the IPP `media-col` attribute.
        let color_media = color_printer
            .as_ref()
            .and_then(|p| p.properties.as_ref())
            .map(|props| props.media_source.clone());

        let mono_media = mono_printer
            .as_ref()
            .and_then(|p| p.properties.as_ref())
            .map(|props| props.media_source.clone());

        (color_printer, mono_printer, color_media, mono_media)
    }
}
