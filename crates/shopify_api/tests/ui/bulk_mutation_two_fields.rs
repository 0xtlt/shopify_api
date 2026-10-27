fn main() {
    let _ = shopify_api::graphql!(bulk("mutation { a: tagsAdd(id: \"gid://shopify/Product/1\", tags: [\"x\"]) { userErrors { message } } b: tagsAdd(id: \"gid://shopify/Product/2\", tags: [\"x\"]) { userErrors { message } } }"));
}
