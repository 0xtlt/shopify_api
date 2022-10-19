#[derive(Clone, Debug)]
pub enum MoneyV2 {
    Amount,
    CurrencyCode,
}

#[derive(Clone, Debug)]
pub enum ProductPriceRangeV2 {
    MaxVariantPrice(Vec<MoneyV2>),
    MinVariantPrice(Vec<MoneyV2>),
}

pub type Product = Vec<ProductEnum>;

#[derive(Clone, Debug)]
pub enum ProductEnum {
    // Fields
    AvailablePublicationCount,
    ContextualPricing(Vec<ProductContextualPricing>),
}

#[derive(Clone, Debug)]
pub enum ProductContextualPricing {
    // Fields
    MaxVariantPricing(Vec<ProductVariantContextualPricing>),
    MinVariantPricing(Vec<ProductVariantContextualPricing>),
    PriceRange(Vec<ProductPriceRangeV2>),
}

#[derive(Clone, Debug)]
pub enum ProductVariantContextualPricing {
    // Fields
    CompareAtPrice(Vec<MoneyV2>),
    Price(Vec<MoneyV2>),
}
