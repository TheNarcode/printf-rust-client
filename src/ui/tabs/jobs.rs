use std::collections::HashMap;
use std::sync::Arc;
use dioxus::prelude::*;
use crate::queue::dispatch::{get_jobs, requeue_to_printer};
use crate::state::AppState;
use crate::types::{ColorMode, JobInfo, Printer};

#[component]
pub fn JobsTab(
    jobs: Signal<Vec<JobInfo>>,
    printers: Signal<Vec<Printer>>,
    now_secs: Signal<u64>,
    mut selected_requeue_printers: Signal<HashMap<String, String>>,
) -> Element {
    let app_state = use_context::<Arc<AppState>>();

    let count_text = {
        let n = jobs().len();
        format!("{} Job{}", n, if n == 1 { "" } else { "s" })
    };

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                if !jobs().is_empty() {
                    div { class: "section-header", style: "justify-content: flex-end; margin-bottom: 0.75rem;",
                        span { class: "badge-count", "{count_text}" }
                    }
                }
                div { class: "jobs-list-container",
                    div { class: "jobs-list",
                        if jobs().is_empty() {
                            div { class: "empty-state",
                                p { "No active print jobs" }
                                span { "Start the client to monitor incoming jobs." }
                            }
                        } else {
                            for job in jobs() {
                                {
                                    let status = job.status.to_lowercase();
                                    let updated_at_u64 = job.updated_at.parse::<u64>().unwrap_or(0);
                                    let limit: Option<u64> = match status.as_str() {
                                        "queued"     => Some(30),
                                        "processing" => Some(120),
                                        _ => None,
                                    };

                                    let elapsed = now_secs().saturating_sub(updated_at_u64);
                                    let is_stuck = status == "stuck" || status == "failed" || (limit.is_some() && elapsed >= limit.unwrap());

                                    let file_id = job.file_id.clone();
                                    let short_id = match &job.order_id {
                                        Some(o) => o.split('-').next().unwrap_or(o).to_string(),
                                        None    => file_id.split('-').next().unwrap_or(&file_id).to_string(),
                                    };

                                    let a = &job.attributes;
                                    let color_str = if a.color == ColorMode::Color { "Color" } else { "B&W" };
                                    let copies_num = a.copies.parse::<i32>().unwrap_or(1);
                                    let num_up    = a.number_up.parse::<i32>().unwrap_or(1);

                                    let state_rq = app_state.clone();
                                    let fid_rq   = file_id.clone();
                                    let fid_sel  = file_id.clone();

                                    let sides_str = match a.sides.as_str() {
                                        "one-sided" => "Single-Sided",
                                        "two-sided-long-edge" | "two-sided-short-edge" | "two-sided" => "Double-Sided",
                                        other if other.contains("two-sided") => "Double-Sided",
                                        other if other.contains("one-sided") => "Single-Sided",
                                        other => other,
                                    };

                                    let page_range_str = if !a.page_ranges.is_empty() && a.page_ranges != "all" && a.page_ranges != "all pages" && a.page_ranges != "1-end" {
                                        format!("Pages {}", a.page_ranges)
                                    } else {
                                        String::new()
                                    };

                                    let orientation_str = match a.orientation.to_lowercase().as_str() {
                                        "portrait" => "Portrait",
                                        "landscape" => "Landscape",
                                        _ => "",
                                    };

                                    let lower_scaling = a.print_scaling.to_lowercase();
                                    let scaling_str = match lower_scaling.as_str() {
                                        "auto" | "none" | "" => "",
                                        "fit" => "Fit to Page",
                                        _ => "",
                                    };

                                    rsx! {
                                        div { class: "job-row-new", key: "{file_id}",
                                            div { class: "job-row-header",
                                                div { style: "display:flex;align-items:center;gap:0.5rem;min-width:0;flex:1",
                                                    span { class: "job-status-dot dot-{status}" }
                                                    span { class: "job-row-title", "{short_id}" }
                                                }
                                                div { class: "job-actions",
                                                    if is_stuck {
                                                        {
                                                            let first_p_name = printers().first().map(|p| p.name.clone()).unwrap_or_default();
                                                            let curr = selected_requeue_printers().get(&fid_sel).cloned().unwrap_or(first_p_name);
                                                            rsx! {
                                                                select {
                                                                    class: "custom-select requeue-select",
                                                                    style: "font-size:0.75rem;padding:0.35rem 1.75rem 0.35rem 0.75rem",
                                                                    value: "{curr}",
                                                                    onchange: {
                                                                        let fid = fid_sel.clone();
                                                                        move |evt: Event<FormData>| {
                                                                            let val = evt.value();
                                                                            let mut map = selected_requeue_printers();
                                                                            map.insert(fid.clone(), val);
                                                                            selected_requeue_printers.set(map);
                                                                        }
                                                                    },
                                                                    for p in printers() {
                                                                        option { value: "{p.name}", "{p.name}" }
                                                                    }
                                                                }
                                                                button {
                                                                    class: "btn btn-primary btn-sm requeue-btn",
                                                                    onclick: {
                                                                        let state = state_rq.clone();
                                                                        let fid   = fid_rq.clone();
                                                                        let target_val = curr.clone();
                                                                        let mut jobs_sig   = jobs;
                                                                        move |_| {
                                                                            let state  = state.clone();
                                                                            let fid    = fid.clone();
                                                                            let target = target_val.clone();
                                                                            spawn(async move {
                                                                                if !target.is_empty() {
                                                                                    let _ = requeue_to_printer(fid, target, Arc::clone(&state)).await;
                                                                                    let updated = get_jobs(Arc::clone(&state)).await;
                                                                                    jobs_sig.set(updated);
                                                                                }
                                                                            });
                                                                        }
                                                                    },
                                                                    "Requeue"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            div { class: "job-pills",
                                                span { class: "pill", "{color_str}" }
                                                if !sides_str.is_empty() { span { class: "pill", "{sides_str}" } }
                                                if copies_num > 1     { span { class: "pill", "×{copies_num}" } }
                                                if num_up > 1         { span { class: "pill", "{num_up}-Up" } }
                                                if !a.paper_format.is_empty()   { span { class: "pill", "{a.paper_format}" } }
                                                if !page_range_str.is_empty()   { span { class: "pill", "{page_range_str}" } }
                                                if !orientation_str.is_empty()  { span { class: "pill", "{orientation_str}" } }
                                                if !scaling_str.is_empty()      { span { class: "pill", "{scaling_str}" } }
                                                if let Some(ref t) = a.target_printer { span { class: "pill", "{t}" } }
                                            }

                                            if let Some(lim) = limit {
                                                {
                                                    let elapsed = now_secs().saturating_sub(updated_at_u64);
                                                    let display_elapsed = elapsed.min(lim);
                                                    rsx! {
                                                        div { class: "job-timer-spinner-wrap",
                                                            if is_stuck {
                                                                span { style: "font-size:1.1rem;line-height:1", "⚠️" }
                                                            } else {
                                                                div { class: "spinner-sm" }
                                                            }
                                                            span { class: "job-timer-label", "{display_elapsed}s / {lim}s" }
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
            }
        }
    }
}