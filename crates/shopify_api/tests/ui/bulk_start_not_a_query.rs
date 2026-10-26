// Anything but a bulk query text or a typed bulk query gets the message of
// `StartBulkQuery`, not one about `From`.
use shopify_api::{Auth, Shopify};

fn main() {
    let shopify = Shopify::new("my-shop", Auth::access_token("shpat_x")).unwrap();
    let _ = shopify.start_bulk_query(42u32);
}
