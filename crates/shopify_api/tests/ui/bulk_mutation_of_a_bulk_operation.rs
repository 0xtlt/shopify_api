fn main() {
    let _ = shopify_api::graphql!(bulk("mutation { bulkOperationRunQuery(query: \"{ shop { name } }\", groupObjects: false) { userErrors { message } } }"));
}
