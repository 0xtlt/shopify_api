fn main() {
    let _ = shopify_api::graphql!(bulk("{ nodes(ids: [\"gid://shopify/Product/1\"]) { id } }"));
}
