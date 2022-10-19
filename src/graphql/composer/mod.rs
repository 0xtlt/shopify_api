// Field returns	Cost value
// Scalar	    0
// Enum	      0
// Object	    1
// Interface	1
// Union	    1
// Mutation	  10
// Connection	2 + 1 per edge (first or last argument)

static SHOPIFY_GRAPHQL_COST_SCALAR: u8 = 0;
static SHOPIFY_GRAPHQL_COST_ENUM: u8 = 0;
static SHOPIFY_GRAPHQL_COST_OBJECT: u8 = 1;
static SHOPIFY_GRAPHQL_COST_INTERFACE: u8 = 1;
static SHOPIFY_GRAPHQL_COST_UNION: u8 = 1;
static SHOPIFY_GRAPHQL_COST_MUTATION: u8 = 10;
static SHOPIFY_GRAPHQL_COST_CONNECTION: u8 = 2;
static SHOPIFY_GRAPHQL_COST_CONNECTION_EDGE: u8 = 1;

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct ShopifyGraphQLComposer {
    /// The generated GraphQL query
    query_str: String,

    /// The query object
    queries: HashMap<String, ShopifyGraphQLComposerQuery>,

    /// The variables listing (required or optionals)
    variables: Vec<ShopifyGraphQLComposerVariableType>,
}

/// Variable type enum
#[derive(Clone, Debug)]
pub enum ShopifyGraphQLComposerVariableType {
    Required(String),
    Optional(String),
}

/// Main Query enum for structure
#[derive(Clone, Debug)]
pub enum ShopifyGraphQLComposerQuery {
    Query(String),
    Mutation(String),
}
