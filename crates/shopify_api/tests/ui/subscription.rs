fn main() {
    let _ = shopify_api::graphql!("subscription { shop { name } }");
}
