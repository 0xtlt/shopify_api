fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id: handle variants { edges { node { sku } } } } } } }"));
}
