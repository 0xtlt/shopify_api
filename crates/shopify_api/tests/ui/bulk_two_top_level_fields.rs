fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id } } } orders { edges { node { id } } } }"));
}
