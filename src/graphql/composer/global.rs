use crate::graphql::composer::ToGraphString;
use graphql_to_string::ToGraphStringDerive;

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum MoneyV2 {
    Amount,
    CurrencyCode,
}

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum PriceRangeV2 {
    MaxVariantPrice(Vec<MoneyV2>),
    MinVariantPrice(Vec<MoneyV2>),
}
