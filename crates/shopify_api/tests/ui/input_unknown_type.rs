fn main() {
    let _ = shopify_api::input!(MetafieldSetInput { ownerId: "gid://shopify/Product/1", key: "color", value: "blue" });
}
