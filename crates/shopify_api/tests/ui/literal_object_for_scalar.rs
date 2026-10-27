fn main() {
    let _ = shopify_api::input!(ProductUpdateInput { id: "gid://shopify/Product/1", title: { value: "Shirt" } });
}
