use super::{Customer, InventoryItem, InventoryLevel, ShopifyWebhook};
use crate::Shopify;
use serde_json;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use warp::http::StatusCode;
use warp::{Filter, Rejection};

impl Shopify {
    #[cfg(feature = "warp-wrapper")]
    pub fn warp_wrapper<F, Fut, T>(
        path: &str,
        shopify_filter: Arc<Mutex<Shopify>>,
        extra_data: T,
        callback: F,
    ) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
    where
        T: Clone + Send + Sync + 'static,
        F: Fn(ShopifyWebhook, Arc<Mutex<Shopify>>, T) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send,
    {
        let path_clone = path.to_string();

        warp::path(path_clone)
            .and(warp::post())
            .and(warp::any().map(move || shopify_filter.clone()))
            .and(warp::any().map(move || extra_data.clone()))
            .and(warp::header::<String>("x-shopify-hmac-sha256"))
            .and(warp::header::<String>("X-Shopify-Topic"))
            .and(warp::body::bytes())
            .and_then(
                move |shopify: Arc<Mutex<Shopify>>,
                      extra: T,
                      header: String,
                      topic: String,
                      body: bytes::Bytes| {
                    let callback_clone = callback.clone();
                    async move {
                        let is_valid = shopify.lock().await.verify_hmac(&body, &header);

                        if !is_valid {
                            return Ok::<_, Rejection>(warp::reply::with_status(
                                warp::reply::html("Invalid HMAC"),
                                StatusCode::BAD_REQUEST,
                            ));
                        }

                        let str_body = match std::str::from_utf8(&body) {
                            Ok(body) => body,
                            Err(_) => {
                                return Ok::<_, Rejection>(warp::reply::with_status(
                                    warp::reply::html("Invalid UTF-8 payload"),
                                    StatusCode::BAD_REQUEST,
                                ));
                            }
                        };

                        log::debug!("Received webhook topic: {}", topic);

                        let webhook_data = match topic.as_str() {
                            "inventory_items/create" => {
                                crate::utils::deserialize_from_str::<InventoryItem>(str_body)
                                    .map(ShopifyWebhook::InventoryItemCreate)
                            }
                            "inventory_items/update" => {
                                crate::utils::deserialize_from_str::<InventoryItem>(str_body)
                                    .map(ShopifyWebhook::InventoryItemUpdate)
                            }
                            "inventory_items/delete" => {
                                crate::utils::deserialize_from_str::<InventoryItem>(str_body)
                                    .map(ShopifyWebhook::InventoryItemDelete)
                            }
                            "inventory_levels/connect" => {
                                crate::utils::deserialize_from_str::<InventoryLevel>(str_body)
                                    .map(ShopifyWebhook::InventoryLevelConnect)
                            }
                            "inventory_levels/disconnect" => {
                                crate::utils::deserialize_from_str::<InventoryLevel>(str_body)
                                    .map(ShopifyWebhook::InventoryLevelDisconnect)
                            }
                            "inventory_levels/update" => {
                                crate::utils::deserialize_from_str::<InventoryLevel>(str_body)
                                    .map(ShopifyWebhook::InventoryLevelUpdate)
                            }
                            "customers/create" => {
                                crate::utils::deserialize_from_str::<Customer>(str_body)
                                    .map(ShopifyWebhook::CustomersCreate)
                            }
                            "customers/update" => {
                                crate::utils::deserialize_from_str::<Customer>(str_body)
                                    .map(ShopifyWebhook::CustomersUpdate)
                            }
                            "orders/create" => crate::utils::deserialize_from_str(str_body)
                                .map(ShopifyWebhook::OrdersCreate),
                            "orders/updated" => crate::utils::deserialize_from_str(str_body)
                                .map(ShopifyWebhook::OrdersUpdated),
                            _ => serde_json::from_str(str_body)
                                .map(|value| ShopifyWebhook::Other((topic.clone(), value)))
                                .map_err(|err| format!("Error parsing JSON: {err}")),
                        };

                        let webhook_data = match webhook_data {
                            Ok(webhook_data) => webhook_data,
                            Err(err) => {
                                log::info!("Failed to parse webhook payload: {}", err);
                                return Ok::<_, Rejection>(warp::reply::with_status(
                                    warp::reply::html("Invalid JSON payload"),
                                    StatusCode::BAD_REQUEST,
                                ));
                            }
                        };

                        match callback_clone(webhook_data, shopify.clone(), extra.clone()).await {
                            Ok(_) => Ok(warp::reply::with_status(
                                warp::reply::html("Success"),
                                StatusCode::OK,
                            )),
                            Err(_e) => {
                                let custom_error = warp::reject::reject();
                                Err(custom_error)
                            }
                        }
                    }
                },
            )
            .boxed()
    }
}
