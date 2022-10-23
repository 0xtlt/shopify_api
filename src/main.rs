use shopify_api::graphql::composer::ToGraphString;
use shopify_api::graphql::composer::{self};

fn main() {
    // let graphql_composer_object = composer::product::Object::ContextualPricing(vec![
    //     composer::product::ContextualPricing::PriceRange(vec![
    //         composer::global::PriceRangeV2::MaxVariantPrice(vec![
    //             composer::global::MoneyV2::Amount,
    //             composer::global::MoneyV2::CurrencyCode,
    //         ]),
    //     ]),
    // ]);
    let graphql_composer_object = composer::product::Object::Description {
        truncate_at: Some(10),
    };

    println!("{}", graphql_composer_object.to_graph_string());
}
