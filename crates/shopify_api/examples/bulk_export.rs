//! Exports every variant of a shop with a bulk operation, outside the rate limit.
//!
//! ```bash
//! SHOPIFY_SHOP=my-shop SHOPIFY_CLIENT_ID=… SHOPIFY_CLIENT_SECRET=… cargo run -p shopify_api --example bulk_export
//! ```

use serde::Deserialize;
use shopify_api::bulk::BulkWaitOptions;
use shopify_api::{Auth, Shopify, graphql};

// Checked against the schema like any other operation; run as a bulk query.
graphql! {
    mod variants = "{
        productVariants {
          edges { node { id sku price inventoryQuantity product { title } } }
        }
    }";
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Variant {
    id: String,
    sku: Option<String>,
    price: String,
    inventory_quantity: Option<i64>,
    product: Product,
}

#[derive(Debug, Deserialize)]
struct Product {
    title: String,
}

#[tokio::main]
async fn main() -> shopify_api::Result<()> {
    let shop = std::env::var("SHOPIFY_SHOP").expect("SHOPIFY_SHOP");
    let client_id = std::env::var("SHOPIFY_CLIENT_ID").expect("SHOPIFY_CLIENT_ID");
    let client_secret = std::env::var("SHOPIFY_CLIENT_SECRET").expect("SHOPIFY_CLIENT_SECRET");
    // The client credentials grant: a store of the organization that owns the app.
    let shopify = Shopify::new(shop, Auth::client_credentials(client_id, client_secret))?;

    // Read as the file downloads: an export of any size, in little memory.
    let mut rows = shopify
        .bulk_query_stream::<Variant>(variants::QUERY, BulkWaitOptions::default())
        .await?;
    let mut count = 0;
    while let Some(variant) = rows.next_row().await {
        let variant = variant?;
        println!(
            "{} {:<12} {:>8} {:>5?}  {}",
            variant.id,
            variant.sku.as_deref().unwrap_or("-"),
            variant.price,
            variant.inventory_quantity,
            variant.product.title
        );
        count += 1;
    }
    println!("{count} variants");
    Ok(())
}
