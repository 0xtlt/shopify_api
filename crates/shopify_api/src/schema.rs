//! Downloading a shop's Admin GraphQL schema, as an introspection result.
//!
//! [`graphql!`](crate::graphql!) checks operations against a crate's schema
//! file, of the version its `Cargo.toml` pins, else of the version that file
//! holds: see [the schema rules](crate::graphql!#schema-and-api-version).
//! `shopify-api schema download` writes that file, from Shopify's public
//! endpoint. [`Shopify::download_admin_schema`] returns the raw
//! introspection result a shop's token sees, for other tools.

use serde_json::{Value, json};

use crate::{Error, Result, Shopify};

/// The introspection query: types, fields, arguments, enum values,
/// directives and deprecations, without descriptions.
pub const INTROSPECTION_QUERY: &str = "\
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    types {
      kind
      name
      fields(includeDeprecated: true) {
        name
        args(includeDeprecated: true) { ...InputValue }
        type { ...TypeRef }
        isDeprecated
        deprecationReason
      }
      inputFields(includeDeprecated: true) { ...InputValue }
      isOneOf
      interfaces { name }
      enumValues(includeDeprecated: true) { name isDeprecated deprecationReason }
      possibleTypes { name }
    }
    directives {
      name
      locations
      isRepeatable
      args(includeDeprecated: true) { ...InputValue }
    }
  }
}
fragment InputValue on __InputValue {
  name
  type { ...TypeRef }
  defaultValue
  isDeprecated
  deprecationReason
}
fragment TypeRef on __Type {
  kind name
  ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } } }
}
";

impl Shopify {
    /// Downloads the Admin schema of the client's version through the shop:
    /// the schema the shop's token sees.
    pub async fn download_admin_schema(&self) -> Result<Value> {
        let raw = self.graphql_raw(INTROSPECTION_QUERY, ()).await?;
        if !raw.errors.is_empty() {
            return Err(Error::Graphql(crate::GraphqlErrors {
                errors: raw.errors,
                data: None,
            }));
        }
        let value = json!({ "data": raw.data });
        if value.pointer("/data/__schema/types").is_none() {
            return Err(Error::decode(
                "data.__schema",
                "the answer is not an introspection result".to_string(),
                None,
            ));
        }
        Ok(value)
    }
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    #[test]
    fn the_query_is_the_one_graphql_uses() {
        assert_eq!(
            super::INTROSPECTION_QUERY,
            shopify_api_graphql::INTROSPECTION_QUERY
        );
    }
}
