//! Downloading Shopify's Admin GraphQL schema, as an introspection result.
//!
//! [`graphql!`](crate::graphql!) checks operations against the schema of
//! the version a crate declares in its `Cargo.toml`, else of the latest
//! version, and downloads it by itself. These functions return the raw
//! introspection result, for other tools.

use serde_json::{Value, json};

use crate::transport::{HttpRequest, Method, Transport};
use crate::{ApiVersion, Error, Result, Shopify};

/// Shopify's public copy of the Admin schema: a GraphQL endpoint per version
/// that answers introspection without a token.
pub const PUBLIC_ADMIN_SCHEMA_ENDPOINT: &str = "https://shopify.dev/admin-graphql-direct-proxy";

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

/// Downloads the Admin schema of `version` from Shopify's public endpoint,
/// with the default transport.
#[cfg(any(feature = "rustls", feature = "native-tls"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "rustls", feature = "native-tls"))))]
pub async fn download_public_admin_schema(version: ApiVersion) -> Result<Value> {
    download_public_admin_schema_with(&crate::transport::ReqwestTransport::new()?, version).await
}

/// Downloads the Admin schema of `version` from Shopify's public endpoint.
pub async fn download_public_admin_schema_with(
    transport: &dyn Transport,
    version: ApiVersion,
) -> Result<Value> {
    let url = format!("{PUBLIC_ADMIN_SCHEMA_ENDPOINT}/{version}");
    let request = HttpRequest::new(Method::Post, &url)
        .with_header("Accept", "application/json")
        .with_header("User-Agent", crate::USER_AGENT)
        .with_json(&json!({ "query": INTROSPECTION_QUERY }))
        .with_timeout(std::time::Duration::from_secs(120));
    let response = transport
        .send(request)
        .await
        .map_err(|source| Error::transport(&url, source))?;
    if !response.is_success() {
        return Err(Error::http(&url, &response));
    }
    let value: Value =
        crate::decode::from_slice(&response.body).map_err(|failure| failure.into_error())?;
    check(value)
}

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
        check(json!({ "data": raw.data }))
    }
}

/// An introspection result, or why it is not one.
fn check(value: Value) -> Result<Value> {
    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Err(Error::http_parts(
            200,
            PUBLIC_ADMIN_SCHEMA_ENDPOINT,
            message.as_bytes(),
        ));
    }
    if value.pointer("/data/__schema/types").is_none() {
        return Err(Error::decode(
            "data.__schema".to_string(),
            "the answer is not an introspection result".to_string(),
            None,
        ));
    }
    Ok(value)
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    #[test]
    fn the_query_and_the_endpoint_are_the_ones_graphql_uses() {
        assert_eq!(
            super::INTROSPECTION_QUERY,
            shopify_api_graphql::INTROSPECTION_QUERY
        );
        assert_eq!(
            super::PUBLIC_ADMIN_SCHEMA_ENDPOINT,
            shopify_api_graphql::project::PUBLIC_ADMIN_SCHEMA_ENDPOINT
        );
    }
}
