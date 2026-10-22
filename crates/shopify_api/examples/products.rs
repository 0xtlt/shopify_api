//! Lists the products of a shop, page after page.
//!
//! ```bash
//! SHOPIFY_SHOP=my-shop SHOPIFY_ACCESS_TOKEN=shpat_… cargo run -p shopify_api --example products
//! ```

use shopify_api::{Auth, Page, Shopify, graphql};

#[tokio::main]
async fn main() -> shopify_api::Result<()> {
    let shop = std::env::var("SHOPIFY_SHOP").expect("SHOPIFY_SHOP");
    let token = std::env::var("SHOPIFY_ACCESS_TOKEN").expect("SHOPIFY_ACCESS_TOKEN");
    let shopify = Shopify::new(shop, Auth::access_token(token))?;

    let products = shopify
        .paginate(
            |after| {
                graphql!(
                    "query Products($after: String) {
                       products(first: 100, after: $after, sortKey: TITLE) {
                         nodes { id title status totalInventory }
                         pageInfo { hasNextPage endCursor }
                       }
                     }",
                    after = after,
                )
            },
            |data| {
                let page_info = data.products.page_info;
                Page::new(
                    data.products.nodes,
                    page_info.has_next_page,
                    page_info.end_cursor,
                )
            },
        )
        .await?;

    for product in &products {
        println!(
            "{:<10} {:>6?}  {}",
            product.status, product.total_inventory, product.title
        );
    }
    println!("{} products", products.len());
    Ok(())
}
