use crate::types::PrintAttributes;

// ─── PDF page slicing ──────────────────────────────────────────────────────────

/// Parses a page-range string (e.g. `"1-3,5,7-9"`) into a sorted list of 1-based
/// page numbers, clamped to `[1, max_pages]`.
fn parse_page_numbers(s: &str, max_pages: u32) -> Vec<u32> {
    let mut result = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.contains('-') {
            let mut iter = part.split('-');
            if let (Some(start_s), Some(end_s)) = (iter.next(), iter.next()) {
                if let (Ok(start), Ok(end)) = (
                    start_s.trim().parse::<u32>(),
                    end_s.trim().parse::<u32>(),
                ) {
                    for page in start..=end {
                        if page >= 1 && page <= max_pages {
                            result.push(page);
                        }
                    }
                }
            }
        } else if let Ok(num) = part.parse::<u32>() {
            if num >= 1 && num <= max_pages {
                result.push(num);
            }
        }
    }
    result
}

/// Extracts the specified pages from a PDF and returns a new PDF containing only
/// those pages.
///
/// If `page_ranges_str` is empty or the range is invalid, the original bytes are
/// returned unchanged.
pub fn slice_pdf_bytes(
    pdf_bytes: &[u8],
    page_ranges_str: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    if page_ranges_str.trim().is_empty() {
        return Ok(pdf_bytes.to_vec());
    }

    let mut doc = lopdf::Document::load_mem(pdf_bytes)?;
    let total_pages = doc.get_pages().len() as u32;
    let selected = parse_page_numbers(page_ranges_str, total_pages);

    if selected.is_empty() {
        return Ok(pdf_bytes.to_vec());
    }

    let all_pages: std::collections::HashSet<u32> = doc.get_pages().keys().cloned().collect();
    let keep_set: std::collections::HashSet<u32> = selected.into_iter().collect();
    let to_delete: Vec<u32> = all_pages.difference(&keep_set).cloned().collect();

    if !to_delete.is_empty() {
        doc.delete_pages(&to_delete);
        doc.prune_objects();
    }

    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

// ─── PDF footer / cover-page processing ───────────────────────────────────────

/// Escapes characters that have special meaning inside PDF string literals.
fn sanitize_pdf_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Prepends `new_page_ids` to the front of the PDF page tree.
fn prepend_pages_to_doc(
    doc: &mut lopdf::Document,
    new_page_ids: Vec<lopdf::ObjectId>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let catalog = doc.catalog()?;
    let pages_id = catalog.get(b"Pages")?.as_reference()?;

    for &page_id in &new_page_ids {
        if let Ok(page_dict) = doc
            .get_object_mut(page_id)
            .and_then(lopdf::Object::as_dict_mut)
        {
            page_dict.set("Parent", pages_id);
        }
    }

    let pages_dict = doc
        .get_object_mut(pages_id)
        .and_then(lopdf::Object::as_dict_mut)?;
    let count = pages_dict.get(b"Count")?.as_i64()? + new_page_ids.len() as i64;
    pages_dict.set("Count", count);

    let kids_obj = pages_dict.get_mut(b"Kids")?;
    let kids_arr = kids_obj.as_array_mut()?;
    let mut new_kids: Vec<lopdf::Object> = new_page_ids
        .into_iter()
        .map(lopdf::Object::Reference)
        .collect();
    new_kids.append(kids_arr);
    *kids_arr = new_kids;

    Ok(())
}

/// Creates a minimal Helvetica Type 1 font object in the document and returns its ID.
fn create_type1_font(doc: &mut lopdf::Document) -> lopdf::ObjectId {
    let mut dict = lopdf::Dictionary::new();
    dict.set("Type", lopdf::Object::Name(b"Font".to_vec()));
    dict.set("Subtype", lopdf::Object::Name(b"Type1".to_vec()));
    dict.set("BaseFont", lopdf::Object::Name(b"Helvetica".to_vec()));
    doc.add_object(dict)
}

/// Ensures that the page's Resources dictionary has a `PrintfFooterFont` entry
/// pointing to a Helvetica font, handling all three resource-dictionary layouts
/// (indirect reference, inline dict, mixed).
fn ensure_font_on_page(doc: &mut lopdf::Document, page_id: lopdf::ObjectId) {
    let font_obj_id = create_type1_font(doc);

    // Case 1: Resources is an indirect reference (e.g. /Resources 488 0 R)
    let res_ref = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|d| d.get(b"Resources").ok())
        .and_then(|o| o.as_reference().ok());

    if let Some(res_id) = res_ref {
        // Sub-case: /Font inside Resources is itself an indirect reference
        let font_ref = doc
            .get_object(res_id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|d| d.get(b"Font").ok())
            .and_then(|o| o.as_reference().ok());

        if let Some(font_id) = font_ref {
            if let Ok(fd) = doc
                .get_object_mut(font_id)
                .and_then(lopdf::Object::as_dict_mut)
            {
                if !fd.has(b"PrintfFooterFont") {
                    fd.set("PrintfFooterFont", font_obj_id);
                }
                return;
            }
        }

        if let Ok(rd) = doc
            .get_object_mut(res_id)
            .and_then(lopdf::Object::as_dict_mut)
        {
            if let Ok(fm) = rd
                .get_mut(b"Font")
                .and_then(lopdf::Object::as_dict_mut)
            {
                if !fm.has(b"PrintfFooterFont") {
                    fm.set("PrintfFooterFont", font_obj_id);
                }
            } else {
                let mut fm = lopdf::Dictionary::new();
                fm.set("PrintfFooterFont", font_obj_id);
                rd.set("Font", fm);
            }
            return;
        }
    }

    // Case 2: Resources is inline on the page dictionary
    if let Ok(pd) = doc
        .get_object_mut(page_id)
        .and_then(lopdf::Object::as_dict_mut)
    {
        if let Ok(rd) = pd
            .get_mut(b"Resources")
            .and_then(lopdf::Object::as_dict_mut)
        {
            if let Ok(fm) = rd
                .get_mut(b"Font")
                .and_then(lopdf::Object::as_dict_mut)
            {
                if !fm.has(b"PrintfFooterFont") {
                    fm.set("PrintfFooterFont", font_obj_id);
                }
            } else {
                let mut fm = lopdf::Dictionary::new();
                fm.set("PrintfFooterFont", font_obj_id);
                rd.set("Font", fm);
            }
        } else {
            let mut fm = lopdf::Dictionary::new();
            fm.set("PrintfFooterFont", font_obj_id);
            let mut rd = lopdf::Dictionary::new();
            rd.set("Font", fm);
            pd.set("Resources", rd);
        }
    }
}

/// Appends a vertical side-bar token overlay to a single PDF page.
fn add_footer_to_page(
    doc: &mut lopdf::Document,
    page_id: lopdf::ObjectId,
    token: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ensure_font_on_page(doc, page_id);

    let q_start_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
        lopdf::Dictionary::new(),
        b"q ".to_vec(),
    )));

    let sanitized = sanitize_pdf_text(token);
    let char_count = token.chars().count() as f64;
    let box_height = (char_count * 6.5) + 12.0;

    let footer_str = format!(
        "q 1 1 1 rg 18.0 30.0 16.0 {:.1} re f Q \
         q 0 0 0 rg 0 0 0 RG BT /PrintfFooterFont 10 Tf \
         0 1 -1 0 24.0 36.0 Tm ({}) Tj ET Q Q",
        box_height, sanitized
    );

    let footer_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
        lopdf::Dictionary::new(),
        footer_str.into_bytes(),
    )));

    if let Ok(page) = doc.get_dictionary(page_id) {
        let mut new_contents = vec![lopdf::Object::Reference(q_start_id)];
        match page.get(b"Contents") {
            Ok(lopdf::Object::Reference(id)) => {
                new_contents.push(lopdf::Object::Reference(*id));
            }
            Ok(lopdf::Object::Array(arr)) => {
                new_contents.extend(arr.clone());
            }
            Ok(lopdf::Object::Stream(stream)) => {
                let sid = doc.add_object(lopdf::Object::Stream(stream.clone()));
                new_contents.push(lopdf::Object::Reference(sid));
            }
            _ => {}
        }
        new_contents.push(lopdf::Object::Reference(footer_id));

        if let Ok(pm) = doc
            .get_object_mut(page_id)
            .and_then(lopdf::Object::as_dict_mut)
        {
            pm.set("Contents", new_contents);
        }
    }

    Ok(())
}

/// Processes the PDF according to the queue token and footer setting:
///
/// - **Footer enabled** (`footer == true` or unset): overlays the token as a
///   vertical side-bar on every page.
/// - **Footer disabled** (`footer == false`): prepends a cover page with the
///   token printed in large text at the centre. The cover page is **always A4**
///   (595.28 × 841.89 pt) — it is a physical separator sheet produced by this
///   print queue system, independent of the job's paper format. Blank padding
///   pages are added as needed so that the first page of the actual document
///   lands on the correct face for duplex/N-up printing.
///
/// If no `queue_token_id` is present the PDF is returned unchanged.
pub fn process_pdf_footer(
    pdf_bytes: &[u8],
    attributes: &PrintAttributes,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let token = attributes
        .queue_token_id
        .as_deref()
        .or_else(|| attributes.order.as_deref())
        .unwrap_or("")
        .trim();

    if token.is_empty() {
        return Ok(pdf_bytes.to_vec());
    }
    let token = token.to_string();

    let footer_enabled = attributes.footer.unwrap_or(true);
    let mut doc = lopdf::Document::load_mem(pdf_bytes)?;

    if footer_enabled {
        // Overlay token as a side-bar on every page
        let pages = doc.get_pages();
        for (_, page_id) in pages {
            let _ = add_footer_to_page(&mut doc, page_id, &token);
        }
    } else {
        // Footer is disabled for this job — prepend a cover/separator page instead.
        //
        // The cover page dimensions are always A4 (595.28 × 841.89 pt).
        // This is intentional and by design: the cover page is a physical
        // separator sheet produced by the printf queue system to identify the
        // customer's job at the collection counter.  It is not a content page
        // and therefore does not need to match the document's paper format.
        let font_obj_id = create_type1_font(&mut doc);

        let font_size: f64 = 36.0;
        // A4 dimensions in PDF points (1/72 inch)
        let page_width: f64 = 595.28;
        let page_height: f64 = 841.89;

        let sanitized_token = sanitize_pdf_text(&token);
        let char_count = token.chars().count() as f64;
        let est_text_width = char_count * (font_size * 0.52);
        let x = ((page_width - est_text_width) / 2.0).max(20.0);
        let y = (page_height / 2.0) - (font_size * 0.3);

        let cover_content = format!(
            "BT /PrintfFooterFont {:.1} Tf {:.2} {:.2} Td ({}) Tj ET",
            font_size, x, y, sanitized_token
        )
        .into_bytes();

        let content_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
            lopdf::Dictionary::new(),
            cover_content,
        )));

        let mut font_map = lopdf::Dictionary::new();
        font_map.set("PrintfFooterFont", font_obj_id);
        let mut res_dict = lopdf::Dictionary::new();
        res_dict.set("Font", font_map);

        let media_box = vec![
            lopdf::Object::Real(0.0f32),
            lopdf::Object::Real(0.0f32),
            lopdf::Object::Real(page_width as f32),
            lopdf::Object::Real(page_height as f32),
        ];

        let mut cover_dict = lopdf::Dictionary::new();
        cover_dict.set("Type", lopdf::Object::Name(b"Page".to_vec()));
        cover_dict.set("MediaBox", media_box.clone());
        cover_dict.set("Contents", content_id);
        cover_dict.set("Resources", res_dict);

        let cover_id = doc.add_object(cover_dict);
        let mut page_ids = vec![cover_id];

        // Add blank padding pages so the actual document starts on the right
        // face for the requested number-up and duplex settings.
        let number_up: usize = attributes.number_up.parse().unwrap_or(1).max(1);
        let sides_mult: usize = if attributes.sides.contains("two-sided") {
            2
        } else {
            1
        };
        let total_cover_pages = number_up * sides_mult;

        for _ in 1..total_cover_pages {
            let blank_id = doc.add_object(lopdf::Object::Stream(lopdf::Stream::new(
                lopdf::Dictionary::new(),
                b"BT ET".to_vec(),
            )));
            let mut blank_dict = lopdf::Dictionary::new();
            blank_dict.set("Type", lopdf::Object::Name(b"Page".to_vec()));
            blank_dict.set("MediaBox", media_box.clone());
            blank_dict.set("Contents", blank_id);
            let blank_page_id = doc.add_object(blank_dict);
            page_ids.push(blank_page_id);
        }

        let _ = prepend_pages_to_doc(&mut doc, page_ids);
    }

    let mut output = Vec::new();
    doc.save_to(&mut output)?;
    Ok(output)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ColorMode, PrintAttributes};

    fn make_attrs(footer: Option<bool>, token: Option<&str>) -> PrintAttributes {
        PrintAttributes {
            file_id: "test".to_string(),
            orientation: "portrait".to_string(),
            color: ColorMode::Color,
            copies: "1".to_string(),
            paper_format: "iso_a4_210x297mm".to_string(),
            page_ranges: String::new(),
            number_up: "1".to_string(),
            sides: "one-sided".to_string(),
            document_format: "application/pdf".to_string(),
            print_scaling: "auto".to_string(),
            target_printer: None,
            order: Some("ord1".to_string()),
            printed: None,
            footer,
            queue_token_id: token.map(|s| s.to_string()),
        }
    }

    #[test]
    fn no_token_returns_original() {
        let bytes = b"%PDF-1.4 test";
        let attrs = make_attrs(Some(true), None);
        // Should return original since there's no token
        let result = process_pdf_footer(bytes, &attrs).unwrap();
        assert_eq!(result, bytes);
    }

    #[test]
    fn capstone_footer_processing() {
        if let Ok(bytes) = std::fs::read("Capstone.pdf") {
            let attrs_on = make_attrs(Some(true), Some("CAPSTONE-TOKEN-123"));
            assert!(
                process_pdf_footer(&bytes, &attrs_on).is_ok(),
                "Footer-enabled path should succeed"
            );

            let attrs_off = make_attrs(Some(false), Some("CAPSTONE-TOKEN-123"));
            let result = process_pdf_footer(&bytes, &attrs_off).unwrap();
            let orig = lopdf::Document::load_mem(&bytes).unwrap();
            let with_cover = lopdf::Document::load_mem(&result).unwrap();
            assert_eq!(
                with_cover.get_pages().len(),
                orig.get_pages().len() + 1,
                "Cover page should add exactly one page"
            );
        }
    }
}
