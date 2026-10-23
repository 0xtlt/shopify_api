#![deny(deprecated)]

fn main() {
    let _ = shopify_api::graphql!("{ product(id: \"gid://shopify/Product/1\") { bodyHtml } }");
}
