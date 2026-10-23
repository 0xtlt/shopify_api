//! Sets a `custom.color` metafield on the products listed on the command
//! line, then archives those without a color.
//!
//! ```bash
//! SHOPIFY_SHOP=my-shop SHOPIFY_ACCESS_TOKEN=shpat_… cargo run -p shopify_api --example metafields -- \
//!     gid://shopify/Product/1=blue gid://shopify/Product/2=
//! ```

use shopify_api::{Auth, Shopify, graphql, input};

graphql! {
    /// Writes metafields and returns what Shopify refused.
    mod metafields_set = "mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) {
            metafields { id key value }
            userErrors { field message }
        }
    }";
    mod shop_name = "query ShopName { shop { name } }";
}

struct Row {
    product_id: String,
    color: String,
}

fn color_metafields(rows: &[Row]) -> Vec<metafields_set::MetafieldsSetInput> {
    let mut metafields = Vec::new();
    for row in rows {
        if row.color.is_empty() {
            continue;
        }
        metafields.push(input!(MetafieldsSetInput {
            ownerId: &row.product_id,
            namespace: "custom",
            key: "color",
            value: &row.color,
            type: "single_line_text_field",
        }));
    }
    metafields
}

#[tokio::main]
async fn main() -> shopify_api::Result<()> {
    let shop = std::env::var("SHOPIFY_SHOP").expect("SHOPIFY_SHOP");
    let token = std::env::var("SHOPIFY_ACCESS_TOKEN").expect("SHOPIFY_ACCESS_TOKEN");
    let shopify = Shopify::new(shop, Auth::access_token(token))?;
    let rows: Vec<Row> = std::env::args()
        .skip(1)
        .filter_map(|argument| {
            let (product_id, color) = argument.split_once('=')?;
            Some(Row {
                product_id: product_id.to_string(),
                color: color.to_string(),
            })
        })
        .collect();

    let shop = shopify.execute(shop_name::operation()).await?;
    println!("{}", shop.shop.name);

    let data = shopify
        .execute(metafields_set::Variables {
            metafields: color_metafields(&rows),
        })
        .await?;
    if let Some(payload) = data.metafields_set {
        for metafield in payload.metafields.into_iter().flatten() {
            println!("{} = {}", metafield.key, metafield.value);
        }
        for error in payload.user_errors {
            eprintln!("{:?}: {}", error.field, error.message);
        }
    }

    for row in rows.iter().filter(|row| row.color.is_empty()) {
        let data = shopify
            .execute(graphql!(
                "mutation($product: ProductUpdateInput!) {
                   productUpdate(product: $product) { userErrors { field message } }
                 }",
                product = { id: &row.product_id, status: ARCHIVED },
            ))
            .await?;
        for error in data
            .product_update
            .into_iter()
            .flat_map(|payload| payload.user_errors)
        {
            eprintln!("{}: {:?}: {}", row.product_id, error.field, error.message);
        }
    }
    Ok(())
}
