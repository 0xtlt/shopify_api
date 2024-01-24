#![cfg(feature = "warp-wrapper")]
pub mod warp;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug)]
pub enum ShopifyWebhook {
    InventoryItemCreate(InventoryItem),
    InventoryItemUpdate(InventoryItem),
    InventoryItemDelete(InventoryItem),
    InventoryLevelConnect(InventoryLevel),
    InventoryLevelDisconnect(InventoryLevel),
    InventoryLevelUpdate(InventoryLevel),
    Other((String, Value)),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InventoryLevel {
    pub inventory_item_id: u64,
    pub location_id: u64,
    pub available: Option<i64>,
    pub updated_at: Option<String>,
    pub admin_graphql_api_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InventoryItem {
    pub id: u64,
    pub sku: String,
    pub created_at: String,
    pub updated_at: String,
    pub requires_shipping: bool,
    pub cost: Option<String>,
    pub country_code_of_origin: Option<String>,
    pub province_code_of_origin: Option<String>,
    pub harmonized_system_code: Option<u64>,
    pub tracked: bool,
    pub country_harmonized_system_codes: Vec<CountryHarmonizedSystemCode>,
    pub admin_graphql_api_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CountryHarmonizedSystemCode {
    pub harmonized_system_code: String,
    pub country_code: String,
}
