use std::sync::Arc;
use std::time::Duration;
use chrono::{Local, TimeZone};
use dioxus::prelude::*;
use crate::queue::dispatch::{get_completed_jobs_today, reprint_job};
use crate::state::AppState;
use crate::types::{ColorMode, JobInfo};

#[component]
pub fn CompletedTab(
    mut completed_jobs: Signal<Vec<JobInfo>>,
) -> Element {
    let app_state = use_context::<Arc<AppState>>();
    let mut completed_search = use_signal(String::new);
    let mut is_refreshing     = use_signal(|| false);

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                div { class: "section-header", style: "margin-bottom: 1rem;",
                    div { class: "section-header-left",
                        input {
                            class: "form-input",
                            r#type: "text",
                            placeholder: "Search Order ID...",
                            style: "width: 260px;",
                            value: completed_search(),
                            oninput: move |evt: Event<FormData>| completed_search.set(evt.value()),
                        }
                    }
                    div { class: "section-header-right",
                        button {
                            class: "btn btn-primary btn-sm",
                            disabled: is_refreshing(),
                            title: "Refresh completed jobs",
                            onclick: {
                                let state = app_state.clone();
                                move |_| {
                                    is_refreshing.set(true);
                                    let state = state.clone();
                                    spawn(async move {
                                        let jobs = get_completed_jobs_today(Arc::clone(&state)).await;
                                        completed_jobs.set(jobs);
                                        tokio::time::sleep(Duration::from_millis(300)).await;
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
                        {
                            let search = completed_search().to_lowercase();
                            let filtered: Vec<_> = completed_jobs()
                                .into_iter()
                                .filter(|job| {
                                    if search.is_empty() { return true; }
                                    let short_id = match &job.order_id {
                                        Some(o) => o.split('-').next().unwrap_or(o).to_string(),
                                        None    => job.file_id.split('-').next().unwrap_or(&job.file_id).to_string(),
                                    };
                                    short_id.to_lowercase().contains(&search)
                                        || job.file_id.to_lowercase().contains(&search)
                                        || job.order_id.as_ref().map(|o| o.to_lowercase().contains(&search)).unwrap_or(false)
                                })
                                .collect();

                            if filtered.is_empty() {
                                rsx! {
                                    div { class: "empty-state",
                                        p {
                                            if completed_jobs().is_empty() {
                                                "No jobs completed today."
                                            } else {
                                                "No completed jobs match your search."
                                            }
                                        }
                                    }
                                }
                            } else {
                                rsx! {
                                    for job in filtered {
                                        {
                                            let file_id  = job.file_id.clone();
                                            let short_id = match &job.order_id {
                                                Some(o) => o.split('-').next().unwrap_or(o).to_string(),
                                                None    => file_id.split('-').next().unwrap_or(&file_id).to_string(),
                                            };

                                            let a = &job.attributes;
                                            let color_str  = if a.color == ColorMode::Color { "Color" } else { "B&W" };
                                            let copies_num = a.copies.parse::<i32>().unwrap_or(1);
                                            let num_up     = a.number_up.parse::<i32>().unwrap_or(1);
                                            let time_str = {
                                                let secs = job.updated_at.parse::<i64>().unwrap_or(0);
                                                if let Some(dt) = Local.timestamp_opt(secs, 0).single() {
                                                    dt.format("%I:%M %p").to_string()
                                                } else {
                                                    String::new()
                                                }
                                            };

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

                                            let state_rp   = app_state.clone();
                                            let fid_rp     = file_id.clone();

                                            rsx! {
                                                div { class: "job-row-new", key: "{file_id}",
                                                    div { class: "job-row-header",
                                                        div { style: "display:flex;align-items:center;gap:0.5rem;min-width:0;flex:1",
                                                            span { class: "job-status-dot dot-completed" }
                                                            span { class: "job-row-title", "{short_id}" }
                                                            if !time_str.is_empty() {
                                                                span { style: "font-size:0.8rem;color:var(--text-muted);margin-left:0.25rem;", "{time_str}" }
                                                            }
                                                        }
                                                        div { class: "job-actions",
                                                            button {
                                                                class: "btn-reprint reprint-btn",
                                                                onclick: {
                                                                    let state    = state_rp.clone();
                                                                    let fid      = fid_rp.clone();
                                                                    let mut jobs_sig = completed_jobs;
                                                                    move |_| {
                                                                        let state = state.clone();
                                                                        let fid   = fid.clone();
                                                                        spawn(async move {
                                                                            let _ = reprint_job(fid, Arc::clone(&state)).await;
                                                                            let updated = get_completed_jobs_today(Arc::clone(&state)).await;
                                                                            jobs_sig.set(updated);
                                                                        });
                                                                    }
                                                                },
                                                                "Reprint"
                                                            }
                                                        }
                                                    }

                                                    div { class: "job-pills",
                                                        span { class: "pill", "{color_str}" }
                                                        if !sides_str.is_empty() { span { class: "pill", "{sides_str}" } }
                                                        if copies_num > 1     { span { class: "pill", "×{copies_num}" } }
                                                        if num_up > 1         { span { class: "pill", "{num_up}-Up" } }
                                                        if !a.paper_format.is_empty()  { span { class: "pill", "{a.paper_format}" } }
                                                        if !page_range_str.is_empty()   { span { class: "pill", "{page_range_str}" } }
                                                        if !orientation_str.is_empty()  { span { class: "pill", "{orientation_str}" } }
                                                        if !scaling_str.is_empty()      { span { class: "pill", "{scaling_str}" } }
                                                        if let Some(ref t) = a.target_printer { span { class: "pill", "{t}" } }
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