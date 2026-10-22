async fn email(shopify: &shopify_api::Shopify) -> shopify_api::Result<String> {
    let data = shopify.execute(shopify_api::graphql!("{ shop { name } }")).await?;
    Ok(data.shop.email)
}

fn main() {}
