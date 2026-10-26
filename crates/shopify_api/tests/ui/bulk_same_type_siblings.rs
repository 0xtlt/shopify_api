fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id custom: metafields(namespace: \"custom\") { edges { node { key } } } other: metafields(namespace: \"other\") { edges { node { key } } } } } } }"));
}
