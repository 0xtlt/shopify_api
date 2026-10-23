fn main() {
    let seo = vec![shopify_api::input!(SEOInput { title: "Shirt" })];
    let _ = shopify_api::graphql!(
        "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { userErrors { message } } }",
        metafields = seo,
    );
}
