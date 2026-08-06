use std::sync::Arc;
use std::time::Duration;
use dioxus::prelude::*;
use crate::api::get_stats;
use crate::state::AppState;


#[component]
pub fn StatsTab(
    mut selected_month: Signal<String>,
    mut stats_json: Signal<serde_json::Value>,
) -> Element {
    let app_state = use_context::<Arc<AppState>>();
    let mut is_refreshing     = use_signal(|| false);
    let mut expand_single     = use_signal(|| false);
    let mut expand_double     = use_signal(|| false);
    let mut expand_settlement = use_signal(|| false);
    let get_metric = |key: &str, field: &str| -> f64 {
        stats_json()
            .get(key)
            .and_then(|v| v.get(field))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };
    let count_1s_mono  = get_metric("b/w single sided", "pages");
    let count_2s_mono  = get_metric("b/w double sided", "pages");
    let count_1s_color = get_metric("color single sided", "pages");
    let count_2s_color = get_metric("color double sided", "pages");
    let price_1s_mono  = count_1s_mono  * 3.0;
    let price_2s_mono  = count_2s_mono  * 2.0;
    let price_1s_color = count_1s_color * 6.0;
    let price_2s_color = count_2s_color * 6.0;
    let total_1s_pages = count_1s_mono + count_1s_color;
    let total_2s_pages = count_2s_mono + count_2s_color;
    let total_mono_pages  = count_1s_mono + count_2s_mono;
    let total_color_pages = count_1s_color + count_2s_color;
    let total_gross = price_1s_mono + price_2s_mono + price_1s_color + price_2s_color;
    let customer_collected = total_gross * 1.05;
    let gateway_fee = customer_collected * 0.005;
    let net_received = customer_collected - gateway_fee;
    let vendor_payable = total_gross * 0.06225;
    let operator_earnings = net_received - vendor_payable;
    let earn_1s_mono  = price_1s_mono  * 0.9825;
    let earn_2s_mono  = price_2s_mono  * 0.9825;
    let earn_1s_color = price_1s_color * 0.9825;
    let earn_2s_color = price_2s_color * 0.9825;
    let earn_1s_total = earn_1s_mono + earn_1s_color;
    let earn_2s_total = earn_2s_mono + earn_2s_color;
    let total_jobs = {
        let mut count = 0.0;
        let sj = stats_json();
        if let Some(val) = sj.get("total_orders")
            .or_else(|| sj.get("total_jobs"))
            .or_else(|| sj.get("orders"))
            .and_then(|v| v.as_f64())
        {
            count = val;
        } else {
            for cat in &["b/w single sided", "b/w double sided", "color single sided", "color double sided"] {
                if let Some(c) = sj.get(*cat) {
                    let j = c.get("jobs")
                        .or_else(|| c.get("count"))
                        .or_else(|| c.get("orders"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    count += j;
                }
            }
        }

        if count == 0.0 {
            if let Ok(store) = app_state.job_store.try_lock() {
                let local_completed = store.values().filter(|j| j.status.to_lowercase() == "completed").count() as f64;
                if local_completed > 0.0 {
                    count = local_completed;
                }
            }
        }
        count
    };

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                div { class: "section-header", style: "margin-bottom: 1rem;",
                    div { class: "section-header-left",
                        select {
                            class: "custom-select",
                            value: selected_month(),
                            onchange: {
                                let state = app_state.clone();
                                move |evt: Event<FormData>| {
                                    let m = evt.value();
                                    selected_month.set(m.clone());
                                    let state = state.clone();
                                    spawn(async move {
                                        if let Ok(data) = get_stats(Some(m), state).await {
                                            stats_json.set(data);
                                        }
                                    });
                                }
                            },
                            option { value: "current", "Current Month" }
                            option { value: "past",    "Previous Month" }
                            option { value: "three",   "Last 3 Months" }
                            option { value: "all",     "All Time" }
                        }
                    }
                    div { class: "section-header-right",
                        button {
                            class: "btn btn-primary btn-sm",
                            disabled: is_refreshing(),
                            title: "Refresh Statistics",
                            onclick: {
                                let state = app_state.clone();
                                move |_| {
                                    is_refreshing.set(true);
                                    let state = state.clone();
                                    let month = selected_month();
                                    spawn(async move {
                                        if let Ok(data) = get_stats(Some(month), state).await {
                                            stats_json.set(data);
                                        }
                                        tokio::time::sleep(Duration::from_millis(300)).await;
                                        is_refreshing.set(false);
                                    });
                                }
                            },
                            if is_refreshing() {
                                span { class: "btn-spin-icon", "↻" }
                                span { "Refreshing..." }
                            } else {
                                span { "Refresh Stats" }
                            }
                        }
                    }
                }

                div { class: "kpi-cards-grid",
                    div { class: "kpi-card",
                        span { class: "kpi-card-label", "Total Earnings" }
                        span { class: "kpi-card-val highlight", "₹{operator_earnings:.2}" }
                    }
                    div { class: "kpi-card",
                        span { class: "kpi-card-label", "B&W Printed" }
                        span { class: "kpi-card-val", "{total_mono_pages:.0}" }
                    }
                    div { class: "kpi-card",
                        span { class: "kpi-card-label", "Color Printed" }
                        span { class: "kpi-card-val", "{total_color_pages:.0}" }
                    }
                    div { class: "kpi-card",
                        span { class: "kpi-card-label", "Orders Completed" }
                        span { class: "kpi-card-val", "{total_jobs:.0}" }
                    }
                }

                div { class: "earnings-breakdown-card",
                    div { class: "breakdown-card-header",
                        div { class: "breakdown-card-title", "Earnings Breakdown" }
                    }
                    div { class: "category-accordion-rows-list",

                        div { class: "category-accordion-row",
                            div {
                                class: "category-main-header",
                                onclick: move |_| expand_single.set(!expand_single()),
                                div { class: "category-main-left",
                                    div {
                                        div { class: "category-name", "Single-Sided Printing" }
                                        div { class: "category-meta", "{total_1s_pages:.0} pages" }
                                    }
                                }
                                div { class: "category-main-right",
                                    span { class: "category-earning-val", "₹{earn_1s_total:.2}" }
                                    svg {
                                        class: if expand_single() { "calc-chevron open" } else { "calc-chevron" },
                                        width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
                                        stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round",
                                        path { d: "M6 9l6 6 6-6" }
                                    }
                                }
                            }
                            div {
                                class: if expand_single() { "category-sub-panel open" } else { "category-sub-panel" },
                                div { class: "category-sub-content",
                                    div { class: "category-sub-card",
                                        div { class: "sub-card-left",
                                            span { class: "category-dot dot-mono" }
                                            span { class: "sub-card-title", "Monochrome (B&W)" }
                                            span { class: "sub-card-count", "{count_1s_mono:.0} pages" }
                                        }
                                        span { class: "sub-card-val", "₹{earn_1s_mono:.2}" }
                                    }
                                    div { class: "category-sub-card",
                                        div { class: "sub-card-left",
                                            span { class: "category-dot dot-color" }
                                            span { class: "sub-card-title", "Color" }
                                            span { class: "sub-card-count", "{count_1s_color:.0} pages" }
                                        }
                                        span { class: "sub-card-val", "₹{earn_1s_color:.2}" }
                                    }
                                }
                            }
                        }

                        div { class: "category-accordion-row",
                            div {
                                class: "category-main-header",
                                onclick: move |_| expand_double.set(!expand_double()),
                                div { class: "category-main-left",
                                    div {
                                        div { class: "category-name", "Double-Sided Printing" }
                                        div { class: "category-meta", "{total_2s_pages:.0} pages" }
                                    }
                                }
                                div { class: "category-main-right",
                                    span { class: "category-earning-val", "₹{earn_2s_total:.2}" }
                                    svg {
                                        class: if expand_double() { "calc-chevron open" } else { "calc-chevron" },
                                        width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
                                        stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round",
                                        path { d: "M6 9l6 6 6-6" }
                                    }
                                }
                            }
                            div {
                                class: if expand_double() { "category-sub-panel open" } else { "category-sub-panel" },
                                div { class: "category-sub-content",
                                    div { class: "category-sub-card",
                                        div { class: "sub-card-left",
                                            span { class: "category-dot dot-mono" }
                                            span { class: "sub-card-title", "Monochrome (B&W)" }
                                            span { class: "sub-card-count", "{count_2s_mono:.0} pages" }
                                        }
                                        span { class: "sub-card-val", "₹{earn_2s_mono:.2}" }
                                    }
                                    div { class: "category-sub-card",
                                        div { class: "sub-card-left",
                                            span { class: "category-dot dot-color" }
                                            span { class: "sub-card-title", "Color" }
                                            span { class: "sub-card-count", "{count_2s_color:.0} pages" }
                                        }
                                        span { class: "sub-card-val", "₹{earn_2s_color:.2}" }
                                    }
                                }
                            }
                        }

                    }
                }

                div { class: "settlement-accordion-card",
                    div {
                        class: "settlement-accordion-trigger",
                        onclick: move |_| expand_settlement.set(!expand_settlement()),
                        div { class: "settlement-trigger-left",
                            div { class: "settlement-title", "Settlement Due to Printf" }
                            div { class: "settlement-subtitle", "Amount payable" }
                        }
                        div { class: "settlement-trigger-right",
                            div { class: "settlement-badge", "₹{vendor_payable:.2}" }
                            svg {
                                class: if expand_settlement() { "calc-chevron open" } else { "calc-chevron" },
                                width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
                                stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round",
                                path { d: "M6 9l6 6 6-6" }
                            }
                        }
                    }
                    div {
                        class: if expand_settlement() { "category-sub-panel open" } else { "category-sub-panel" },
                        div { class: "category-sub-content",
                            div { class: "calc-item-row",
                                span { "Total Value" }
                                span { "₹{total_gross:.2}" }
                            }
                            div { class: "calc-item-row",
                                span { "Net Amount" }
                                span { "₹{net_received:.2}" }
                            }
                            div { class: "calc-item-row settlement-highlight",
                                span { "Printf Platform Share" }
                                span { class: "settlement-final-val", "₹{vendor_payable:.2}" }
                            }
                        }
                    }
                }
            }
        }
    }
}