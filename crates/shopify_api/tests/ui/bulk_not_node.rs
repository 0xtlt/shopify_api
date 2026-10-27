fn main() {
    let _ = shopify_api::graphql!(bulk("{ deletionEvents { edges { node { occurredAt } } } }"));
}
