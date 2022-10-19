use crate::graphql::composer::ToGraphString;
use graphql_to_string::ToGraphStringDerive;

use super::global::{MoneyV2, PriceRangeV2};

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum Object {
    // Fields
    AvailablePublicationCount,
    ContextualPricing(Vec<ContextualPricing>),
}

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum ContextualPricing {
    // Fields
    MaxVariantPricing(Vec<VariantContextualPricing>),
    MinVariantPricing(Vec<VariantContextualPricing>),
    PriceRange(Vec<PriceRangeV2>),
}

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum VariantContextualPricing {
    // Fields
    CompareAtPrice(Vec<MoneyV2>),
    Price(Vec<MoneyV2>),
}
