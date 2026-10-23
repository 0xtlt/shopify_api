fn main() {
    let _ = shopify_api::graphql!(
        "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
        product = { id: "gid://shopify/Product/1", titel: "Shirt" },
    );
}
