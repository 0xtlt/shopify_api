fn main() {
    let vendor: Option<String> = None;
    let _ = shopify_api::input!(ProductUpdateInput { id: "gid://shopify/Product/1", vendor: vendor });
}
