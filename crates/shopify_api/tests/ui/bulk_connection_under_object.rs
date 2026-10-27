fn main() {
    let _ = shopify_api::graphql!(bulk("{ orders { edges { node { id customer { orders { edges { node { id } } } } } } } }"));
}
