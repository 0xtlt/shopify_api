fn main() {
    let handle: Option<&str> = Some("shirt");
    let _ = shopify_api::input!(ProductSetIdentifiers { id: "gid://shopify/Product/1", handle: "shirt" });
    let _ = shopify_api::input!(ProductSetIdentifiers {});
    let _ = shopify_api::input!(ProductSetIdentifiers { handle?: handle });
    let _ = shopify_api::input!(ProductSetIdentifiers { handle: null });
    let _ = shopify_api::input!(ProductSetIdentifiers { handle: handle });
    let _ = shopify_api::input!(ProductSetIdentifiers { handle: shopify_api::Maybe::<&str>::Null });
}
