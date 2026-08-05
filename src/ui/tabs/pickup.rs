use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;

use crate::api::{get_completed_orders, mark_order_collected};
use crate::state::AppState;
use crate::types::ApiOrder;

/// The Pickup tab. Shows orders that are paid/printed and ready for customer collection
/// (API status != 3). The operator clicks "Mark Collected" when the customer picks up.
///
/// Orders disappear automatically on next refresh once collected (status becomes 3).
#[component]
pub fn PickupTab(
    mut completed_orders: Signal<Vec<ApiOrder>>,
    mut completed_search: Signal<String>,
) -> Element {
    let app_state = use_context::<Arc<AppState>>();
    let mut is_refreshing = use_signal(|| false);

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                div { class: "completed-orders-section",
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
                                title: "Refresh Pickup Orders",
                                onclick: {
                                    let state = app_state.clone();
                                    move |_| {
                                        is_refreshing.set(true);
                                        let state = state.clone();
                                        spawn(async move {
                                            if let Ok(orders) = get_completed_orders(state).await {
                                                completed_orders.set(orders);
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
                                    span { "Refresh" }
                                }
                            }
                        }
                    }

                    div { class: "jobs-list-container",
                        div { class: "jobs-list",
                            {
                                let search = completed_search().to_lowercase();
                                let filtered: Vec<_> = completed_orders()
                                    .into_iter()
                                    .filter(|o| {
                                        search.is_empty()
                                            || o.id.to_lowercase().contains(&search)
                                    })
                                    .collect();

                                if filtered.is_empty() {
                                    rsx! {
                                        div { class: "empty-state",
                                            p {
                                                if completed_orders().is_empty() {
                                                    "No orders ready for pickup."
                                                } else {
                                                    "No orders match your search."
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    rsx! {
                                        for order in filtered {
                                            {
                                                let order_id   = order.id.clone();
                                                let state_mark = app_state.clone();

                                                rsx! {
                                                    div { class: "completed-order-row", key: "{order_id}",
                                                        div { class: "job-info",
                                                            div { class: "job-id", "Order #{order_id}" }
                                                            div { class: "job-meta", "Ready for pickup" }
                                                        }
                                                        div { class: "job-actions",
                                                            button {
                                                                class: "btn btn-primary btn-sm",
                                                                onclick: {
                                                                    let state = state_mark.clone();
                                                                    let oid   = order_id.clone();
                                                                    move |_| {
                                                                        let state = state.clone();
                                                                        let oid   = oid.clone();
                                                                        spawn(async move {
                                                                            if mark_order_collected(oid, state.clone()).await.is_ok() {
                                                                                if let Ok(orders) = get_completed_orders(state).await {
                                                                                    completed_orders.set(orders);
                                                                                }
                                                                            }
                                                                        });
                                                                    }
                                                                },
                                                                "Mark Collected"
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
}
