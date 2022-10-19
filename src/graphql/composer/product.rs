use crate::graphql::composer::ToGraphString;
use graphql_to_string::ToGraphStringDerive;

use super::global::{Metafield, MoneyV2, PriceRangeV2};

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum Object {
    // Fields
    AvailablePublicationCount,
    ContextualPricing(Vec<ContextualPricing>),
    CreatedAt,
    DefaultCursor,
    // first option is truncate_at
    Description {
        truncate_at: Option<u32>,
    },
    DescriptionHtml,
    FeaturedImage,
    // feedback
    GiftCardTemplateSuffix,
    Handle,
    HasOnlyDefaultVariant,
    HasOutOfStockVariants,
    Id,
    InCollection {
        id: String,
    },
    IsGiftCard,
    LegacyResourceId,
    MediaCount,
    Metafield {
        connector: Metafield,
        namespace: String,
        key: String,
    },
    OnlineStorePreviewUrl,
    Options {
        first: Option<u32>,
    },
    PriceRangeV2(Vec<PriceRangeV2>),
    PrivateMetafield {
        connector: Metafield,
        namespace: String,
        key: String,
    },
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
