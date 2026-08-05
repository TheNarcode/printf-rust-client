use std::sync::Arc;

use dioxus::prelude::*;

use crate::api::get_stats;
use crate::state::AppState;

/// The Statistics tab. Renders revenue and page-count data for the selected time window.
///
/// **Bug 6 fix**: the month dropdown now uses the static values that the printfs API
/// accepts (`"current"`, `"past"`, `"three"`, `"all"`). The previous implementation
/// generated dynamic `"YYYY-MM"` strings that the API's Zod validator always rejected.
#[component]
pub fn StatsTab(
    mut selected_month: Signal<String>,
    mut stats_json: Signal<serde_json::Value>,
) -> Element {
    let app_state = use_context::<Arc<AppState>>();

    let get_count = |key: &str| -> f64 {
        stats_json()
            .get(key)
            .and_then(|v| v.get("pages"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };

    let count_1s_mono  = get_count("b/w single sided");
    let count_2s_mono  = get_count("b/w double sided");
    let count_1s_color = get_count("color single sided");
    let count_2s_color = get_count("color double sided");

    let price_1s_mono  = count_1s_mono  * 3.0;
    let price_2s_mono  = count_2s_mono  * 2.0;
    let price_1s_color = count_1s_color * 6.0;
    let price_2s_color = count_2s_color * 6.0;

    let net_1s_mono  = price_1s_mono  * 0.975;
    let net_2s_mono  = price_2s_mono  * 0.975;
    let net_1s_color = price_1s_color * 0.975;
    let net_2s_color = price_2s_color * 0.975;

    let total_count = count_1s_mono + count_2s_mono + count_1s_color + count_2s_color;
    let total_price = price_1s_mono + price_2s_mono + price_1s_color + price_2s_color;
    let total_net   = net_1s_mono   + net_2s_mono   + net_1s_color   + net_2s_color;
    let vendor_payable = 2.0 * (total_price - total_net);

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                div { class: "section-header",
                    h2 { "Print Statistics & Revenue" }
                    div { class: "stats-controls",
                        button {
                            class: "btn btn-primary btn-sm",
                            title: "Refresh Statistics",
                            onclick: {
                                let state = app_state.clone();
                                move |_| {
                                    let state = state.clone();
                                    let month = selected_month();
                                    spawn(async move {
                                        if let Ok(data) = get_stats(Some(month), state).await {
                                            stats_json.set(data);
                                        }
                                    });
                                }
                            },
                            "Refresh Stats"
                        }
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
                }

                div { class: "stats-container",
                    div { class: "stat-header-row",
                        div { class: "stat-col-header",              "Category" }
                        div { class: "stat-col-header text-right",   "Pages Printed" }
                        div { class: "stat-col-header text-right",   "Gross (100%)" }
                        div { class: "stat-col-header text-right",   "Net Earning (97.5%)" }
                    }
                    div { class: "stat-rows-group",
                        div { class: "stat-row",
                            div { class: "stat-category-label",
                                span { class: "category-dot dot-mono-1" }
                                span { "1 Sided Monochrome" }
                            }
                            div { class: "stat-val text-right", "{count_1s_mono}" }
                            div { class: "stat-val text-right", span { "₹{price_1s_mono:.2}" } }
                            div { class: "stat-val text-right stat-highlight", span { "₹{net_1s_mono:.2}" } }
                        }
                        div { class: "stat-row",
                            div { class: "stat-category-label",
                                span { class: "category-dot dot-mono-2" }
                                span { "2 Sided Monochrome" }
                            }
                            div { class: "stat-val text-right", "{count_2s_mono}" }
                            div { class: "stat-val text-right", span { "₹{price_2s_mono:.2}" } }
                            div { class: "stat-val text-right stat-highlight", span { "₹{net_2s_mono:.2}" } }
                        }
                        div { class: "stat-row",
                            div { class: "stat-category-label",
                                span { class: "category-dot dot-color-1" }
                                span { "1 Sided Color" }
                            }
                            div { class: "stat-val text-right", "{count_1s_color}" }
                            div { class: "stat-val text-right", span { "₹{price_1s_color:.2}" } }
                            div { class: "stat-val text-right stat-highlight", span { "₹{net_1s_color:.2}" } }
                        }
                        div { class: "stat-row",
                            div { class: "stat-category-label",
                                span { class: "category-dot dot-color-2" }
                                span { "2 Sided Color" }
                            }
                            div { class: "stat-val text-right", "{count_2s_color}" }
                            div { class: "stat-val text-right", span { "₹{price_2s_color:.2}" } }
                            div { class: "stat-val text-right stat-highlight", span { "₹{net_2s_color:.2}" } }
                        }
                    }
                    div { class: "stat-total-row",
                        div { class: "stat-total-label",                          "Total Revenue" }
                        div { class: "stat-total-val text-right",                 "{total_count}" }
                        div { class: "stat-total-val text-right",     span { "₹{total_price:.2}" } }
                        div { class: "stat-total-val text-right stat-total-highlight", span { "₹{total_net:.2}" } }
                    }
                }

                div { class: "vendor-payable-container",
                    div { class: "vendor-payable-info",
                        div { class: "vendor-payable-icon",
                            svg {
                                class: "bi bi-currency-dollar",
                                view_box: "0 0 16 16", width: "16", height: "16", fill: "currentColor",
                                path { d: "M4 10.781c.148 1.667 1.513 2.85 3.591 3.003V15h1.043v-1.216c2.27-.179 3.678-1.438 3.678-3.3 0-1.59-.947-2.51-2.956-3.028l-.722-.187V3.467c1.122.11 1.879.714 2.07 1.616h1.47c-.166-1.6-1.54-2.748-3.54-2.875V1H7.591v1.233c-1.939.23-3.27 1.472-3.27 3.156 0 1.454.966 2.483 2.661 2.917l.61.162v4.031c-1.149-.17-1.94-.8-2.131-1.718zm3.391-3.836c-1.043-.263-1.6-.825-1.6-1.616 0-.944.704-1.641 1.8-1.828v3.495l-.2-.05zm1.591 1.872c1.287.323 1.852.859 1.852 1.769 0 1.097-.826 1.828-2.2 1.939V8.73z" }
                            }
                        }
                        div { class: "vendor-payable-text", h3 { "Vendor Payable Amount" } }
                    }
                    div { class: "vendor-payable-amount", "₹{vendor_payable:.2}" }
                }
            }
        }
    }
}
