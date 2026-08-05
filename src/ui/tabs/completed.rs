use std::sync::Arc;

use dioxus::prelude::*;

use crate::api::{get_completed_orders, mark_order_collected};
use crate::state::AppState;
use crate::types::ApiOrder;

/// The Completed Orders tab. Shows only orders that are paid/printed and
/// have NOT yet been collected by the customer (status != 3).
///
/// This is intentionally a "Ready for Pickup" view — once an order is marked
/// collected it is removed from the list automatically on next refresh.
#[component]
pub fn CompletedTab(
    mut completed_orders: Signal<Vec<ApiOrder>>,
    mut completed_search: Signal<String>,
) -> Element {
    let app_state = use_context::<Arc<AppState>>();

    rsx! {
        div { class: "page-view active",
            section { class: "section-jobs",
                div { class: "completed-orders-section",
                    div { class: "section-header", style: "margin-bottom: 1rem;",
                        h2 { "Completed Orders" }
                        div { style: "display:flex;gap:0.5rem;align-items:center;",
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "Search Order ID...",
                                style: "width: 210px; height: 32px;",
                                value: completed_search(),
                                oninput: move |evt: Event<FormData>| completed_search.set(evt.value()),
                            }
                            button {
                                class: "btn btn-primary btn-sm",
                                title: "Refresh Completed Orders",
                                onclick: {
                                    let state = app_state.clone();
                                    move |_| {
                                        let state = state.clone();
                                        spawn(async move {
                                            if let Ok(orders) = get_completed_orders(state).await {
                                                completed_orders.set(orders);
                                            }
                                        });
                                    }
                                },
                                "Refresh"
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
                                                // After filtering in api::get_completed_orders we
                                                // should never see status==3 here, but guard anyway
                                                let is_collected = order.status == Some(3);
                                                let state_mark = app_state.clone();

                                                rsx! {
                                                    div { class: "completed-order-row", key: "{order_id}",
                                                        div { class: "job-info",
                                                            div { class: "job-id", "Order #{order_id}" }
                                                            div { class: "job-meta",
                                                                if is_collected { "Collected" } else { "Ready for pickup" }
                                                            }
                                                        }
                                                        div { class: "job-actions",
                                                            if !is_collected {
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
                                                            } else {
                                                                span {
                                                                    style: "color:#10b981;font-size:0.85rem;font-weight:600;",
                                                                    "✓ Collected"
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
}
