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
    CustomersCreate(Customer),
    CustomersUpdate(Customer),
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
pub struct Address {
    pub id: u64,
    pub customer_id: u64,
    pub first_name: String,
    pub last_name: String,
    pub company: Option<String>,
    pub address1: Option<String>,
    pub address2: Option<String>,
    pub city: Option<String>,
    pub province: Option<String>,
    pub country: String,
    pub zip: Option<String>,
    pub phone: Option<String>,
    pub province_code: Option<String>,
    pub country_code: String,
    pub country_name: String,
    pub default: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Customer {
    pub id: u64,
    pub email: String,
    pub created_at: String,
    pub updated_at: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub orders_count: u64,
    pub state: String,
    pub total_spent: String,
    pub last_order_id: Option<u64>,
    pub note: Option<String>,
    pub verified_email: bool,
    pub multipass_identifier: Option<String>,
    pub tax_exempt: bool,
    pub tags: String,
    pub last_order_name: Option<String>,
    pub phone: Option<String>,
    pub addresses: Vec<Address>,
    pub admin_graphql_api_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CountryHarmonizedSystemCode {
    pub harmonized_system_code: String,
    pub country_code: String,
}
