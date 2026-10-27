fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id collections { edges { node { id products { edges { node { id variants { edges { node { sku } } } } } } } } } } } } }"));
}
