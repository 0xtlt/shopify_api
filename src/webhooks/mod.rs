pub mod verify;
pub mod webhook;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ShopifyWebhook {
    id: u64,
    address: String,
    topic: String,
    created_at: String,
    updated_at: String,
    format: String,
    fields: Vec<String>,
    metafield_namespaces: Vec<String>,
    api_version: String,
    private_metafield_namespaces: Vec<String>,
}
