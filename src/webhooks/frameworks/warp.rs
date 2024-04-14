use super::{Customer, InventoryItem, InventoryLevel, ShopifyWebhook};
use crate::Shopify;
use serde_json;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use warp::{http::StatusCode, Filter, Rejection};

impl Shopify {
    #[cfg(feature = "warp-wrapper")]
    pub fn warp_wrapper<F, Fut>(
        path: &str,
        shopify_filter: Arc<Mutex<Shopify>>,
        callback: F,
    ) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
    where
        F: Fn(ShopifyWebhook, Arc<Mutex<Shopify>>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send,
    {
        let path_clone = path.to_string();

        warp::path(path_clone)
            .and(warp::post())
            .and(warp::any().map(move || shopify_filter.clone()))
            .and(warp::header::<String>("x-shopify-hmac-sha256"))
            .and(warp::header::<String>("X-Shopify-Topic"))
            .and(warp::body::bytes())
            .and_then(
                move |shopify: Arc<Mutex<Shopify>>,
                      header: String,
                      topic: String,
                      body: bytes::Bytes| {
                    let callback_clone = callback.clone();
                    async move {
                        let is_valid = shopify.lock().await.verify_hmac(&body, &header);

                        if !is_valid {
                            log::info!("Invalid HMAC");
                            return Ok::<_, Rejection>(warp::reply::with_status(
                                warp::reply::html("Invalid HMAC"),
                                StatusCode::BAD_REQUEST,
                            ));
                        }

                        let str_body = std::str::from_utf8(&body).unwrap();

                        let webhook_data: ShopifyWebhook = match topic.as_str() {
                            "inventory_items/create" => ShopifyWebhook::InventoryItemCreate(
                                serde_json::from_str::<InventoryItem>(str_body).unwrap(),
                            ),
                            "inventory_items/update" => ShopifyWebhook::InventoryItemUpdate(
                                serde_json::from_str::<InventoryItem>(str_body).unwrap(),
                            ),
                            "inventory_items/delete" => ShopifyWebhook::InventoryItemDelete(
                                serde_json::from_str::<InventoryItem>(str_body).unwrap(),
                            ),
                            "inventory_levels/connect" => ShopifyWebhook::InventoryLevelConnect(
                                serde_json::from_str::<InventoryLevel>(str_body).unwrap(),
                            ),
                            "inventory_levels/disconnect" => {
                                ShopifyWebhook::InventoryLevelDisconnect(
                                    serde_json::from_str::<InventoryLevel>(str_body).unwrap(),
                                )
                            }
                            "inventory_levels/update" => ShopifyWebhook::InventoryLevelUpdate(
                                serde_json::from_str::<InventoryLevel>(str_body).unwrap(),
                            ),
                            "customers/create" => ShopifyWebhook::CustomersCreate(
                                serde_json::from_str::<Customer>(str_body).unwrap(),
                            ),
                            "customers/update" => ShopifyWebhook::CustomersUpdate(
                                serde_json::from_str::<Customer>(str_body).unwrap(),
                            ),
                            "orders/create" => ShopifyWebhook::OrdersCreate(
                                serde_json::from_str(str_body).unwrap(),
                            ),
                            _ => ShopifyWebhook::Other((
                                topic,
                                serde_json::from_str(str_body).unwrap(),
                            )),
                        };

                        match callback_clone(webhook_data, shopify.clone()).await {
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
