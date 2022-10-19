use super::global::{MoneyV2, PriceRangeV2};

#[derive(Clone, Debug)]
pub enum Object {
    // Fields
    AvailablePublicationCount,
    ContextualPricing(Vec<ContextualPricing>),
}

// impl ShopifyGraphQLComposerObject for Object {
//     fn to_graph_string(&self) -> String {
//         match self {
//             Object::AvailablePublicationCount => "availablePublicationCount".to_string(),
//             Object::ContextualPricing(fields) => {
//                 let mut s = String::new();
//                 s.push_str("contextualPricing");
//                 s.push_str(" { ");
//                 for field in fields {
//                     s.push_str(&field.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//         }
//     }
// }

#[derive(Clone, Debug)]
pub enum ContextualPricing {
    // Fields
    MaxVariantPricing(Vec<VariantContextualPricing>),
    MinVariantPricing(Vec<VariantContextualPricing>),
    PriceRange(Vec<PriceRangeV2>),
}

// impl ShopifyGraphQLComposerObject for ContextualPricing {
//     fn to_graph_string(&self) -> String {
//         match self {
//             ContextualPricing::MaxVariantPricing(fields) => {
//                 let mut s = String::new();
//                 s.push_str("maxVariantPricing");
//                 s.push_str(" { ");
//                 for field in fields {
//                     s.push_str(&field.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//             ContextualPricing::MinVariantPricing(fields) => {
//                 let mut s = String::new();
//                 s.push_str("minVariantPricing");
//                 s.push_str(" { ");
//                 for field in fields {
//                     s.push_str(&field.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//             ContextualPricing::PriceRange(fields) => {
//                 let mut s = String::new();
//                 s.push_str("priceRange");
//                 s.push_str(" { ");
//                 for field in fields {
//                     s.push_str(&field.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//         }
//     }
// }

#[derive(Clone, Debug)]
pub enum VariantContextualPricing {
    // Fields
    CompareAtPrice(Vec<MoneyV2>),
    Price(Vec<MoneyV2>),
}

// impl ShopifyGraphQLComposerObject for VariantContextualPricing {
//     fn to_graph_string(&self) -> String {
//         match self {
//             VariantContextualPricing::CompareAtPrice(fields) => {
//                 let mut s = String::new();
//                 s.push_str("compareAtPrice");
//                 s.push_str(" { ");
//                 for field in fields {
//                     s.push_str(&field.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//             VariantContextualPricing::Price(fields) => {
//                 let mut s = String::new();
//                 s.push_str("price");
//                 s.push_str(" { ");
//                 for field in fields {
//                     s.push_str(&field.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//         }
//     }
// }
