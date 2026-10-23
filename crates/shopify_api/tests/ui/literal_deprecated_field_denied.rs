#![deny(deprecated)]

fn main() {
    let _ = shopify_api::input!(ProductVariantsBulkInput { barcode: "1234", price: "10.00" });
}
