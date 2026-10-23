// An alias spelled like its field, with the fields of an input object in
// another order: Shopify (GraphQL Ruby) refuses it with `fieldConflict`.
fn main() {
    let _ = shopify_api::graphql!(
        r#"mutation { draftOrderCreate:draftOrderCreate(input:{note:"x" tags:["y"]}){draftOrder{id}} draftOrderCreate:draftOrderCreate(input:{tags:["y"] note:"x"}){draftOrder{id}} }"#
    );
}
