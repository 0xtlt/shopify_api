#![cfg(feature = "warp-wrapper")]
pub mod warp;

// https://shopify.dev/docs/api/admin-rest/2024-04/resources/webhook#event-topics

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
    OrdersCreate(Order),
    OrdersUpdated(Order),
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
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub company: Option<String>,
    pub address1: Option<String>,
    pub address2: Option<String>,
    pub city: Option<String>,
    pub province: Option<String>,
    pub country: Option<String>,
    pub zip: Option<String>,
    pub phone: Option<String>,
    pub province_code: Option<String>,
    pub country_code: Option<String>,
    pub country_name: Option<String>,
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
    pub orders_count: Option<u64>,
    pub state: String,
    pub total_spent: Option<String>,
    pub last_order_id: Option<u64>,
    pub note: Option<String>,
    pub verified_email: bool,
    pub multipass_identifier: Option<String>,
    pub tax_exempt: bool,
    pub tags: Option<String>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Order {
    pub id: u64,
    pub admin_graphql_api_id: String,
    pub app_id: Option<u64>,
    pub browser_ip: Option<String>,
    pub buyer_accepts_marketing: bool,
    pub cancel_reason: Option<String>,
    pub cancelled_at: Option<String>,
    pub cart_token: Option<String>,
    pub checkout_id: Option<u64>,
    pub checkout_token: Option<String>,
    pub client_details: Option<OrderClientDetails>,
    pub closed_at: Option<String>,
    pub confirmation_number: Option<String>,
    pub confirmed: bool,
    pub contact_email: String,
    pub created_at: String,
    pub currency: String,
    pub current_subtotal_price: String,
    pub current_subtotal_price_set: OrderPriceSet,
    pub current_total_additional_fees_set: Option<OrderAdditionalFeesSet>,
    pub current_total_discounts: String,
    pub current_total_discounts_set: OrderPriceSet,
    pub current_total_duties_set: Option<OrderDutiesSet>,
    pub current_total_price: String,
    pub current_total_price_set: OrderPriceSet,
    pub current_total_tax: String,
    pub current_total_tax_set: OrderPriceSet,
    pub customer_locale: Option<String>,
    pub device_id: Option<u64>,
    pub discount_codes: Vec<OrderDiscountCode>,
    pub email: String,
    pub estimated_taxes: bool,
    pub financial_status: String,
    pub fulfillment_status: Option<String>,
    pub landing_site: Option<String>,
    pub landing_site_ref: Option<String>,
    pub location_id: Option<u64>,
    pub merchant_of_record_app_id: Option<u64>,
    pub name: String,
    pub note: Option<String>,
    pub note_attributes: Vec<OrderNoteAttribute>,
    pub number: u64,
    pub order_number: u64,
    pub order_status_url: String,
    pub original_total_additional_fees_set: Option<OrderAdditionalFeesSet>,
    pub original_total_duties_set: Option<OrderDutiesSet>,
    pub payment_gateway_names: Vec<String>,
    pub phone: Option<String>,
    pub po_number: Option<String>,
    pub presentment_currency: String,
    pub processed_at: Option<String>,
    pub reference: Option<String>,
    pub referring_site: Option<String>,
    pub source_identifier: Option<String>,
    pub source_name: String,
    pub source_url: Option<String>,
    pub subtotal_price: String,
    pub subtotal_price_set: OrderPriceSet,
    pub tags: String,
    pub tax_exempt: bool,
    pub tax_lines: Vec<OrderTaxLine>,
    pub taxes_included: bool,
    pub test: bool,
    pub token: String,
    pub total_discounts: String,
    pub total_discounts_set: OrderPriceSet,
    pub total_line_items_price: String,
    pub total_line_items_price_set: OrderPriceSet,
    pub total_outstanding: String,
    pub total_price: String,
    pub total_price_set: OrderPriceSet,
    pub total_shipping_price_set: OrderPriceSet,
    pub total_tax: String,
    pub total_tax_set: OrderPriceSet,
    pub total_tip_received: String,
    pub total_weight: u64,
    pub updated_at: String,
    pub user_id: Option<u64>,
    pub billing_address: Option<OrderAddress>,
    pub customer: OrderCustomer,
    pub discount_applications: Vec<OrderDiscountApplication>,
    pub fulfillments: Vec<OrderFulfillment>,
    pub line_items: Vec<OrderLineItem>,
    pub payment_terms: Option<OrderPaymentTerms>,
    pub refunds: Vec<OrderRefund>,
    pub shipping_address: Option<OrderAddress>,
    pub shipping_lines: Vec<OrderShippingLine>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderClientDetails {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderPriceSet {
    pub shop_money: OrderMoney,
    pub presentment_money: OrderMoney,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderMoney {
    pub amount: String,
    pub currency_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderAdditionalFeesSet {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderDutiesSet {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderDiscountCode {
    pub code: Option<String>,
    pub amount: Option<String>,
    #[serde(rename = "type")]
    pub _type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderNoteAttribute {
    pub name: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderTaxLine {
    pub title: Option<String>,
    pub price: Option<String>,
    pub rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderAddress {
    pub first_name: Option<String>,
    pub address1: Option<String>,
    pub phone: Option<String>,
    pub city: Option<String>,
    pub zip: Option<String>,
    pub province: Option<String>,
    pub country: Option<String>,
    pub last_name: Option<String>,
    pub address2: Option<String>,
    pub company: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub name: Option<String>,
    pub country_code: Option<String>,
    pub province_code: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderCustomer {
    pub id: u64,
    pub email: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub state: Option<String>,
    pub note: Option<String>,
    pub verified_email: bool,
    pub multipass_identifier: Option<String>,
    pub tax_exempt: bool,
    pub phone: Option<String>,
    pub email_marketing_consent: Option<OrderConsent>,
    pub sms_marketing_consent: Option<OrderConsent>,
    pub tags: Option<String>,
    pub currency: String,
    pub tax_exemptions: Vec<String>,
    pub admin_graphql_api_id: String,
    pub default_address: Option<OrderAddress>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderConsent {
    pub state: Option<String>,
    pub opt_in_level: Option<String>,
    pub consent_updated_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderDiscountApplication {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderFulfillment {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderLineItem {
    pub id: u64,
    pub admin_graphql_api_id: Option<String>,
    pub variant_id: Option<u64>,
    pub quantity: i32,
    pub price: String,
    pub grams: i32,
    pub name: String,
    pub title: String,
    pub variant_title: Option<String>,
    pub sku: Option<String>,
    pub variant_inventory_management: Option<String>,
    pub product_id: Option<u64>,
    pub fulfillment_service: String,
    pub product_exists: bool,
    pub taxable: bool,
    pub total_discount: String,
    pub fulfillment_status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderPaymentTerms {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderRefund {}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderShippingLine {
    pub id: u64,
    pub title: String,
    pub price: String,
    pub code: Option<String>,
    pub source: String,
    pub phone: Option<String>,
    pub requested_fulfillment_service_id: Option<String>,
    pub delivery_category: Option<String>,
    pub carrier_identifier: Option<String>,
    pub discounted_price: String,
}
