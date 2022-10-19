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

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum Image {
    AltText,
    Height,
    Id,
    Metafield {
        connector: Metafield,
        namespace: String,
        key: String,
    },
    PrivateMetafield {
        connector: Metafield,
        namespace: String,
        key: String,
    },
    Url,
    Width,
}

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum Metafield {
    CreatedAt,
    Definition,
}

#[derive(Clone, Debug)]
pub enum MetafieldValidationStatus {
    Valid,
    Invalid,
    ANY,
}

#[derive(Clone, Debug)]
pub enum MetafieldOwnerType {
    ApiPermission,
    Article,
    Blog,
    Collection,
    Customer,
    Discount,
    DraftOrder,
    Location,
    Order,
    Page,
    Product,
    ProductVariant,
    Shop,
}

#[derive(Clone, Debug, ToGraphStringDerive)]
pub enum MetafieldDefinition {
    Description,
    Id,
    Key,
    MetafieldsCount {
        validation_status: Option<MetafieldValidationStatus>,
    },
    Name,
    Namespace,
    OwnerType,
    PinnedPosition,
    // standardTemplate
    // type
    // validationStatus
    // validations
    // visibleToStorefrontApi
}
