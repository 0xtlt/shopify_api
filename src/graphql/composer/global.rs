use graphql_to_string::ToGraphStringDerive;
use crate::graphql::composer::ToGraphString;

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum MoneyV2 {
    Amount,
    CurrencyCode,
}

// impl ToGraphString for MoneyV2 {
//     fn to_graph_string(&self) -> String {
//         match self {
//             MoneyV2::Amount => "amount".to_string(),
//             MoneyV2::CurrencyCode => "currencyCode".to_string(),
//         }
//     }
// }

// pub fn generate_wrapped_object(name: &str, fields: Vec<MoneyV2>) -> String {
//     let mut s = String::new();
//     s.push_str(name);
//     s.push_str(" { ");
//     for field in fields.iter().enumerate() {
//         if field.0 > 0 {
//             s.push('\n');
//         }
//         s.push_str(&field.1.to_graph_string());
//     }
//     s.push_str(" }");
//     s
// }

#[derive(Clone, Debug)]
pub enum PriceRangeV2 {
    MaxVariantPrice(Vec<MoneyV2>),
    MinVariantPrice(Vec<MoneyV2>),
}

// impl ToGraphString for PriceRangeV2 {
//     fn to_graph_string(&self) -> String {
//         match self {
//             PriceRangeV2::MaxVariantPrice(fields) => {
//                 let mut s = String::new();
//                 s.push_str("maxVariantPrice");
//                 s.push_str(" { ");
//                 for field in fields.iter().enumerate() {
//                     if field.0 > 0 {
//                         s.push('\n');
//                     }
//                     s.push_str(&field.1.to_graph_string());
//                 }
//                 s.push_str(" }");
//                 s
//             }
//             PriceRangeV2::MinVariantPrice(fields) => {
//                 let mut s = String::new();
//                 s.push_str("minVariantPrice");
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
