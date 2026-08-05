use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use dioxus::desktop::use_window;
use dioxus::prelude::*;

use crate::api::{get_completed_orders, get_stats};
use crate::printer::client::get_printer_list;
use crate::queue::dispatch::{get_jobs, start_client, stop_client};
use crate::state::{AppState, current_timestamp_secs};
use crate::types::{ApiOrder, JobInfo, Printer};
use crate::ui::tabs::{
    completed::CompletedTab, jobs::JobsTab, settings::SettingsTab, stats::StatsTab,
};

/// The four top-level navigation tabs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Jobs,
    Stats,
    Completed,
    Settings,
}

/// Root Dioxus component. Owns all application-level reactive signals and
/// drives the background polling loop.
#[component]
pub fn App() -> Element {
    let window    = use_window();
    let app_state = use_context::<Arc<AppState>>();

    // ── Top-level signals ──────────────────────────────────────────────────────
    let mut is_running       = use_signal(|| app_state.is_running.load(Ordering::SeqCst));
    let mut active_tab       = use_signal(|| Tab::Jobs);
    let mut jobs             = use_signal(Vec::<JobInfo>::new);
    let mut printers         = use_signal(Vec::<Printer>::new);
    let mut completed_orders = use_signal(Vec::<ApiOrder>::new);
    let completed_search     = use_signal(String::new);
    let selected_month       = use_signal(|| "current".to_string());
    let mut stats_json       = use_signal(|| serde_json::Value::Null);
    let mut now_secs         = use_signal(current_timestamp_secs);
    let selected_requeue_printers = use_signal(HashMap::<String, String>::new);

    // ── Initial printer fetch ──────────────────────────────────────────────────
    let state_init = app_state.clone();
    use_future(move || {
        let state = state_init.clone();
        async move {
            if let Ok(list) = get_printer_list(state).await {
                printers.set(list);
            }
        }
    });

    // ── 1-second heartbeat: clock, is_running flag, job list ──────────────────
    let state_timer = app_state.clone();
    use_future(move || {
        let state = state_timer.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                now_secs.set(current_timestamp_secs());
                let running = state.is_running.load(Ordering::SeqCst);
                if is_running() != running {
                    is_running.set(running);
                }
                let current_jobs = get_jobs(state.clone()).await;
                jobs.set(current_jobs);
            }
        }
    });

    // ── Tab-change side effects: load tab-specific data ────────────────────────
    let state_tab = app_state.clone();
    use_effect(move || {
        let tab   = active_tab();
        let month = selected_month();
        let state = state_tab.clone();
        spawn(async move {
            match tab {
                Tab::Jobs => {}
                Tab::Stats => {
                    if let Ok(data) = get_stats(Some(month), state).await {
                        stats_json.set(data);
                    }
                }
                Tab::Completed => {
                    if let Ok(orders) = get_completed_orders(state).await {
                        completed_orders.set(orders);
                    }
                }
                Tab::Settings => {
                    if let Ok(list) = get_printer_list(state).await {
                        printers.set(list);
                    }
                }
            }
        });
    });

    let jobs_count_text = {
        let n = jobs().len();
        format!("{} Job{}", n, if n == 1 { "" } else { "s" })
    };

    let window_drag_topbar = window.clone();
    let state_toggle       = app_state.clone();

    let tab_title = match active_tab() {
        Tab::Jobs => "Active Print Jobs",
        Tab::Stats => "Print Statistics & Revenue",
        Tab::Completed => "Completed Orders",
        Tab::Settings => "Printer Settings",
    };

    rsx! {
        style { {include_str!("../../ui/styles.css")} }
        div { class: "app-container",

            // ══════════════════════════════════════════════════════════════════
            // LEFT SIDEBAR NAVIGATION
            // ══════════════════════════════════════════════════════════════════
            aside { class: "app-sidebar",
                // Sidebar Nav Links
                div { class: "sidebar-nav-container",
                    div { class: "sidebar-nav-title", "Menu" }

                    // 1. Jobs Tab
                    button {
                        class: if active_tab() == Tab::Jobs { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                        onclick: move |_| active_tab.set(Tab::Jobs),
                        div { class: "sidebar-nav-item-left",
                            svg {
                                width: "16", height: "16", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor",
                                stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                path { d: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" }
                                polyline { points: "14 2 14 8 20 8" }
                                line { x1: "16", y1: "13", x2: "8", y2: "13" }
                                line { x1: "16", y1: "17", x2: "8", y2: "17" }
                                polyline { points: "10 9 9 9 8 9" }
                            }
                            span { "Active Jobs" }
                        }
                        if !jobs().is_empty() {
                            span { class: "sidebar-nav-badge", "{jobs_count_text}" }
                        }
                    }

                    // 2. Stats Tab
                    button {
                        class: if active_tab() == Tab::Stats { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                        onclick: move |_| active_tab.set(Tab::Stats),
                        div { class: "sidebar-nav-item-left",
                            svg {
                                width: "16", height: "16", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor",
                                stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                line { x1: "18", y1: "20", x2: "18", y2: "10" }
                                line { x1: "12", y1: "20", x2: "12", y2: "4" }
                                line { x1: "6",  y1: "20", x2: "6",  y2: "14" }
                            }
                            span { "Statistics" }
                        }
                    }

                    // 3. Completed Tab
                    button {
                        class: if active_tab() == Tab::Completed { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                        onclick: move |_| active_tab.set(Tab::Completed),
                        div { class: "sidebar-nav-item-left",
                            svg {
                                width: "16", height: "16", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor",
                                stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                polyline { points: "9 11 12 14 22 4" }
                                path { d: "M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" }
                            }
                            span { "Completed" }
                        }
                    }

                    // 4. Settings Tab
                    button {
                        class: if active_tab() == Tab::Settings { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                        onclick: move |_| active_tab.set(Tab::Settings),
                        div { class: "sidebar-nav-item-left",
                            svg {
                                width: "16", height: "16", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor",
                                stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                circle { cx: "12", cy: "12", r: "3" }
                                path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
                            }
                            span { "Settings" }
                        }
                    }
                }
            }

            // ══════════════════════════════════════════════════════════════════
            // RIGHT MAIN CONTENT AREA
            // ══════════════════════════════════════════════════════════════════
            div { class: "app-body",
                // Top Draggable Window Bar
                header {
                    class: "top-title-bar",
                    onmousedown: move |_| { window_drag_topbar.drag(); },
                    div { class: "top-title-bar-left",
                        span { class: "top-title-bar-text", "{tab_title}" }
                    }
                    div {
                        class: "top-title-bar-right",
                        onmousedown: move |evt| evt.stop_propagation(),
                        button {
                            class: if is_running() { "btn btn-danger btn-sm" } else { "btn btn-primary btn-sm" },
                            onmousedown: move |evt| evt.stop_propagation(),
                            onclick: {
                                let state = state_toggle.clone();
                                move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    let state = state.clone();
                                    spawn(async move {
                                        let currently_running = state.is_running.load(Ordering::SeqCst);
                                        if currently_running {
                                            if let Ok(msg) = stop_client(state.clone()).await {
                                                log::info!("{}", msg);
                                            }
                                        } else {
                                            if let Ok(msg) = start_client(state.clone()).await {
                                                log::info!("{}", msg);
                                            }
                                        }
                                        is_running.set(state.is_running.load(Ordering::SeqCst));
                                    });
                                }
                            },
                            if is_running() { "Stop Client" } else { "Start Client" }
                        }
                    }
                }

                // Main Tab View Area
                main { class: "main-content",
                    match active_tab() {
                        Tab::Jobs => rsx! {
                            JobsTab {
                                jobs: jobs,
                                printers: printers,
                                now_secs: now_secs,
                                selected_requeue_printers: selected_requeue_printers,
                            }
                        },
                        Tab::Stats => rsx! {
                            StatsTab {
                                selected_month: selected_month,
                                stats_json: stats_json,
                            }
                        },
                        Tab::Completed => rsx! {
                            CompletedTab {
                                completed_orders: completed_orders,
                                completed_search: completed_search,
                            }
                        },
                        Tab::Settings => rsx! {
                            SettingsTab {
                                printers: printers,
                            }
                        },
                    }
                }
            }
        }
    }
}
