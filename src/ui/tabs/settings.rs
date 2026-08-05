use std::sync::Arc;

use dioxus::prelude::*;

use crate::printer::client::{
    add_appsocket_printer, delete_printer, fetch_printer_properties, get_cups_ppds,
    get_printer_list, pause_printer, save_printer_properties, unpause_printer, CupsPpdModel,
};
use crate::state::AppState;
use crate::types::{ColorMode, Printer, PrinterProperties};

/// The Settings tab. Displays the CUPS printer list with pause/resume and
/// property editing. Also owns the "Add Printer" and "Edit Properties" modals.
///
/// Modals use `position: fixed` so they render on top of all content regardless
/// of their position in the DOM.
#[component]
pub fn SettingsTab(mut printers: Signal<Vec<Printer>>) -> Element {
    let app_state = use_context::<Arc<AppState>>();
    let mut is_refreshing = use_signal(|| false);

    // ── Add-printer modal signals ──────────────────────────────────────────────
    let mut show_add_modal = use_signal(|| false);
    let mut new_name       = use_signal(String::new);
    let mut new_ip         = use_signal(String::new);
    let mut new_port       = use_signal(|| "9100".to_string());
    let mut new_color      = use_signal(|| ColorMode::Color);
    let mut new_ppd_choice     = use_signal(|| "raw".to_string());
    let mut canon_models       = use_signal(Vec::<CupsPpdModel>::new);
    let mut selected_canon_ppd = use_signal(String::new);
    let mut is_loading_canon   = use_signal(|| false);
    let mut uploaded_ppd_name  = use_signal(String::new);
    let mut uploaded_ppd_bytes = use_signal(|| None::<Vec<u8>>);
    let mut add_status_msg     = use_signal(String::new);

    // ── Edit-properties modal signals ──────────────────────────────────────────
    let mut editing_printer   = use_signal(|| None::<Printer>);
    let mut edit_media        = use_signal(|| "iso_a4_210x297mm".to_string());
    let mut edit_media_source = use_signal(|| "auto".to_string());
    let mut edit_orientation  = use_signal(|| "portrait".to_string());
    let mut edit_print_quality = use_signal(|| "normal".to_string());
    let mut edit_sides        = use_signal(|| "one-sided".to_string());
    let mut edit_color        = use_signal(|| ColorMode::Color);

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                div { class: "section-header", style: "margin-bottom: 1rem;",
                    div { class: "section-header-left",
                        button {
                            class: "btn btn-primary btn-sm",
                            onclick: move |_| {
                                add_status_msg.set(String::new());
                                show_add_modal.set(true);
                            },
                            "+ Add New Printer"
                        }
                    }
                    div { class: "section-header-right",
                        button {
                            class: "btn btn-primary btn-sm",
                            disabled: is_refreshing(),
                            onclick: {
                                let state = app_state.clone();
                                move |_| {
                                    is_refreshing.set(true);
                                    let state = state.clone();
                                    spawn(async move {
                                        if let Ok(list) = get_printer_list(state).await {
                                            printers.set(list);
                                        }
                                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                        is_refreshing.set(false);
                                    });
                                }
                            },
                            if is_refreshing() {
                                span { class: "btn-spin-icon", "↻" }
                                span { "Refreshing..." }
                            } else {
                                span { "Refresh" }
                            }
                        }
                    }
                }

                div { class: "jobs-list-container",
                    div { class: "jobs-list",
                        if printers().is_empty() {
                            div { class: "empty-state", p { "No printers found" } }
                        } else {
                            for p in printers() {
                                {
                                    let is_paused   = p.paused;
                                    let uri         = p.uri.clone();
                                    let state_tog   = app_state.clone();
                                    let printer_obj = p.clone();

                                    rsx! {
                                        div { class: "printer-card", key: "{uri}",
                                            div { class: "printer-card-info",
                                                div { style: "display:flex;align-items:center;gap:0.5rem;",
                                                    span { class: if is_paused { "job-status-dot dot-stuck" } else { "job-status-dot dot-completed" } }
                                                    div { class: "printer-card-name", "{p.name}" }
                                                }
                                            }
                                            div { style: "display:flex;gap:0.5rem;align-items:center;",
                                                // Properties button — fetches live attrs then opens modal
                                                button {
                                                    class: "btn-reprint",
                                                    style: "font-size:0.75rem;padding:0.35rem 0.75rem;",
                                                    onclick: {
                                                        let state = app_state.clone();
                                                        let po    = printer_obj.clone();
                                                        move |_| {
                                                            let state = state.clone();
                                                            let po    = po.clone();
                                                            spawn(async move {
                                                                let (fetched_props, fetched_color) =
                                                                    fetch_printer_properties(&po.name, state).await;
                                                                edit_media.set(fetched_props.media.clone());
                                                                edit_media_source.set(fetched_props.media_source.clone());
                                                                edit_orientation.set(fetched_props.orientation.clone());
                                                                edit_print_quality.set(fetched_props.print_quality.clone());
                                                                edit_sides.set(fetched_props.sides.clone());
                                                                edit_color.set(fetched_color.clone());
                                                                let mut updated = po.clone();
                                                                updated.properties = Some(fetched_props);
                                                                updated.color_mode = fetched_color;
                                                                editing_printer.set(Some(updated));
                                                            });
                                                        }
                                                    },
                                                    "Properties"
                                                }
                                                // Pause / Resume toggle
                                                button {
                                                    class: if is_paused { "printer-toggle-btn paused" } else { "printer-toggle-btn active" },
                                                    onclick: {
                                                        let state      = state_tog.clone();
                                                        let target_uri = uri.clone();
                                                        move |_| {
                                                            let state      = state.clone();
                                                            let target_uri = target_uri.clone();
                                                            spawn(async move {
                                                                if is_paused {
                                                                    let _ = unpause_printer(target_uri, state.clone()).await;
                                                                } else {
                                                                    let _ = pause_printer(target_uri, state.clone()).await;
                                                                }
                                                                if let Ok(list) = get_printer_list(state).await {
                                                                    printers.set(list);
                                                                }
                                                            });
                                                        }
                                                    },
                                                    if is_paused { "Resume" } else { "Pause" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── Modal: Add New AppSocket Printer ────────────────────────────────────
        if show_add_modal() {
            div { class: "modal-backdrop",
                onclick: move |_| show_add_modal.set(false),
                div { class: "modal-content",
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        h3 { "Add New AppSocket Printer" }
                        button {
                            class: "modal-close-btn",
                            onclick: move |_| show_add_modal.set(false),
                            "×"
                        }
                    }
                    div { class: "modal-body",
                        if !add_status_msg().is_empty() {
                            div { style: "color:var(--destructive);font-size:0.8rem;font-weight:500;", "{add_status_msg}" }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Printer Name (CUPS Identifier)" }
                            input {
                                class: "form-input", r#type: "text",
                                placeholder: "e.g. office_jet_9100",
                                value: new_name(),
                                oninput: move |e: Event<FormData>| new_name.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Printer IP Address / Host" }
                            input {
                                class: "form-input", r#type: "text",
                                placeholder: "e.g. 192.168.1.100",
                                value: new_ip(),
                                oninput: move |e: Event<FormData>| new_ip.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "AppSocket Port (Default: 9100)" }
                            input {
                                class: "form-input", r#type: "number",
                                placeholder: "9100",
                                value: new_port(),
                                oninput: move |e: Event<FormData>| new_port.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Color Capability" }
                            select {
                                class: "custom-select",
                                value: if new_color() == ColorMode::Color { "color" } else { "monochrome" },
                                onchange: move |e: Event<FormData>| {
                                    new_color.set(if e.value() == "color" { ColorMode::Color } else { ColorMode::Monochrome });
                                },
                                option { value: "color",      "Color Printer" }
                                option { value: "monochrome", "Monochrome (B&W) Printer" }
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "PPD Driver / Make & Model" }
                            select {
                                class: "custom-select",
                                value: new_ppd_choice(),
                                onchange: {
                                    let state = app_state.clone();
                                    move |e: Event<FormData>| {
                                        let choice = e.value();
                                        new_ppd_choice.set(choice.clone());
                                        if choice == "canon" && canon_models().is_empty() {
                                            is_loading_canon.set(true);
                                            let state = state.clone();
                                            spawn(async move {
                                                let models = get_cups_ppds(Some("Canon".to_string()), &state).await;
                                                if let Some(first) = models.first() {
                                                    selected_canon_ppd.set(first.ppd_name.clone());
                                                }
                                                canon_models.set(models);
                                                is_loading_canon.set(false);
                                            });
                                        }
                                    }
                                },
                                option { value: "raw",    "RAW Printer (Pass-through)" }
                                option { value: "canon",  "Canon Models (Installed on Device)" }
                                option { value: "upload", "Custom PPD File (Upload .ppd)" }
                            }
                        }
                        if new_ppd_choice() == "canon" {
                            div { class: "form-group",
                                label { class: "form-label", "Select Canon Model" }
                                if is_loading_canon() {
                                    div { style: "font-size:0.8rem;color:var(--text-muted);padding:0.5rem 0;", "Fetching installed Canon models..." }
                                } else if canon_models().is_empty() {
                                    div { style: "font-size:0.8rem;color:var(--text-muted);padding:0.5rem 0;", "No Canon models found on host. Use Custom PPD File below to upload your printer's .ppd file." }
                                } else {
                                    select {
                                        class: "custom-select",
                                        value: selected_canon_ppd(),
                                        onchange: move |e: Event<FormData>| selected_canon_ppd.set(e.value()),
                                        for model in canon_models() {
                                            option {
                                                key: "{model.ppd_name}",
                                                value: "{model.ppd_name}",
                                                "{model.description}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if new_ppd_choice() == "upload" {
                            div { class: "form-group",
                                label { class: "form-label", "Or Provide a PPD File" }
                                input {
                                    class: "form-input",
                                    r#type: "file",
                                    accept: ".ppd",
                                    onchange: move |evt: Event<FormData>| {
                                        spawn(async move {
                                            if let Some(file_engine) = evt.files() {
                                                let files = file_engine.files();
                                                if let Some(first_file) = files.first() {
                                                    let fname = first_file.clone();
                                                    if let Some(bytes) = file_engine.read_file(&fname).await {
                                                        uploaded_ppd_name.set(fname);
                                                        uploaded_ppd_bytes.set(Some(bytes));
                                                    }
                                                }
                                            }
                                        });
                                    }
                                }
                                if !uploaded_ppd_name().is_empty() {
                                    div { style: "color:#10b981;font-size:0.75rem;font-weight:500;margin-top:0.35rem;",
                                        "✓ Loaded PPD File: {uploaded_ppd_name}"
                                    }
                                }
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn-reprint",
                            onclick: move |_| show_add_modal.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            onclick: {
                                let state = app_state.clone();
                                move |_| {
                                    let name  = new_name();
                                    let ip    = new_ip();
                                    let port  = new_port().parse::<u16>().unwrap_or(9100);
                                    let color = new_color();
                                    let (ppd_name, ppd_bytes) = match new_ppd_choice().as_str() {
                                        "raw" => (Some("raw".to_string()), None),
                                        "canon" => {
                                            let p = selected_canon_ppd();
                                            if p.is_empty() {
                                                add_status_msg.set("Please select a Canon model".to_string());
                                                return;
                                            }
                                            (Some(p), None)
                                        }
                                        "upload" => {
                                            if let (Some(bytes), fname) = (uploaded_ppd_bytes(), uploaded_ppd_name()) {
                                                if !bytes.is_empty() && !fname.is_empty() {
                                                    (None, Some(bytes))
                                                } else {
                                                    add_status_msg.set("Please select a valid .ppd file to upload".to_string());
                                                    return;
                                                }
                                            } else {
                                                add_status_msg.set("Please select a .ppd file to upload".to_string());
                                                return;
                                            }
                                        }
                                        _ => (Some("raw".to_string()), None),
                                    };
                                    let state = state.clone();
                                    spawn(async move {
                                        match add_appsocket_printer(name, ip, port, color, ppd_name, ppd_bytes, state.clone()).await {
                                            Ok(_) => {
                                                show_add_modal.set(false);
                                                new_name.set(String::new());
                                                new_ip.set(String::new());
                                                new_port.set("9100".to_string());
                                                uploaded_ppd_name.set(String::new());
                                                uploaded_ppd_bytes.set(None);
                                                if let Ok(list) = get_printer_list(state).await {
                                                    printers.set(list);
                                                }
                                            }
                                            Err(err) => add_status_msg.set(err),
                                        }
                                    });
                                }
                            },
                            "Add Printer"
                        }
                    }
                }
            }
        }

        // ── Modal: Edit Printer Properties ─────────────────────────────────────
        if let Some(target_p) = editing_printer() {
            div { class: "modal-backdrop",
                onclick: move |_| editing_printer.set(None),
                div { class: "modal-content",
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        h3 { "Printer Properties — {target_p.name}" }
                        button {
                            class: "modal-close-btn",
                            onclick: move |_| editing_printer.set(None),
                            "×"
                        }
                    }
                    div { class: "modal-body",
                        div { class: "form-group",
                            label { class: "form-label", "Default Paper Size (Media)" }
                            select {
                                class: "custom-select",
                                value: edit_media(),
                                onchange: move |e: Event<FormData>| edit_media.set(e.value()),
                                option { value: "iso_a4_210x297mm",  "A4 (210 x 297 mm)" }
                                option { value: "na_letter_8.5x11in", "US Letter (8.5 x 11 in)" }
                                option { value: "iso_a3_297x420mm",  "A3 (297 x 420 mm)" }
                                option { value: "na_legal_8.5x14in", "US Legal (8.5 x 14 in)" }
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Default Input Tray / Media Source" }
                            select {
                                class: "custom-select",
                                value: edit_media_source(),
                                onchange: move |e: Event<FormData>| edit_media_source.set(e.value()),
                                option { value: "auto",    "Auto Select" }
                                option { value: "main",    "Main Tray" }
                                option { value: "top",     "Top Tray" }
                                option { value: "bottom",  "Bottom Tray" }
                                option { value: "tray-1",  "Tray 1" }
                                option { value: "tray-2",  "Tray 2" }
                                option { value: "manual",  "Manual Feed" }
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Default Orientation" }
                            select {
                                class: "custom-select",
                                value: edit_orientation(),
                                onchange: move |e: Event<FormData>| edit_orientation.set(e.value()),
                                option { value: "portrait",  "Portrait" }
                                option { value: "landscape", "Landscape" }
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Default Duplex / Sides" }
                            select {
                                class: "custom-select",
                                value: edit_sides(),
                                onchange: move |e: Event<FormData>| edit_sides.set(e.value()),
                                option { value: "one-sided",            "Single-Sided (Simplex)" }
                                option { value: "two-sided-long-edge",  "Double-Sided (Long Edge)" }
                                option { value: "two-sided-short-edge", "Double-Sided (Short Edge)" }
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Color Mode" }
                            select {
                                class: "custom-select",
                                value: if edit_color() == ColorMode::Color { "color" } else { "monochrome" },
                                onchange: move |e: Event<FormData>| {
                                    edit_color.set(if e.value() == "color" { ColorMode::Color } else { ColorMode::Monochrome });
                                },
                                option { value: "color",      "Color" }
                                option { value: "monochrome", "Monochrome (B&W)" }
                            }
                        }
                    }
                    div { class: "modal-footer", style: "display:flex;justify-content:space-between;align-items:center;",
                        button {
                            class: "btn btn-primary",
                            style: "background:#ef4444;border-color:#ef4444;color:#ffffff;",
                            onclick: {
                                let state = app_state.clone();
                                let target_name = target_p.name.clone();
                                move |_| {
                                    let name  = target_name.clone();
                                    let state = state.clone();
                                    spawn(async move {
                                        let _ = delete_printer(name, state.clone()).await;
                                        editing_printer.set(None);
                                        if let Ok(list) = get_printer_list(state).await {
                                            printers.set(list);
                                        }
                                    });
                                }
                            },
                            "Remove Printer"
                        }
                        div { style: "display:flex;gap:0.5rem;align-items:center;",
                            button {
                                class: "btn-reprint",
                                onclick: move |_| editing_printer.set(None),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: {
                                    let state       = app_state.clone();
                                    let target_name = target_p.name.clone();
                                    move |_| {
                                        let props = PrinterProperties {
                                            media:         edit_media(),
                                            media_source:  edit_media_source(),
                                            orientation:   edit_orientation(),
                                            print_quality: edit_print_quality(),
                                            sides:         edit_sides(),
                                        };
                                        let color       = edit_color();
                                        let name        = target_name.clone();
                                        let state       = state.clone();
                                        spawn(async move {
                                            let _ = save_printer_properties(name, props, color, state.clone()).await;
                                            editing_printer.set(None);
                                            if let Ok(list) = get_printer_list(state).await {
                                                printers.set(list);
                                            }
                                        });
                                    }
                                },
                                "Save Properties"
                            }
                        }
                    }
                }
            }
        }
    }
}
