mod bulk_query;
use crate::{
    utils::{self, read_json_tree, ReadJsonTreeSteps},
    Shopify, ShopifyAPIError,
};
use reqwest::Response;

async fn shopify_graphql_query<VariablesType, ReturnType>(
    (shopify, graphql_query, variables, json_finder): &(
        &Shopify,
        &str,
        &VariablesType,
        &Vec<ReadJsonTreeSteps<'_>>,
    ),
) -> Result<ReturnType, ShopifyAPIError>
where
    VariablesType: serde::Serialize,
    ReturnType: serde::de::DeserializeOwned,
{
    // Prepare the client
    let client = reqwest::Client::new();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("X-Shopify-Access-Token", shopify.api_key.parse().unwrap());
    let req_body: &serde_json::Value = &serde_json::json!({
        "query": graphql_query,
        "variables": variables
    });

    // Connection Response
    let res: Response = client
        .post(shopify.get_query_url())
        .headers(headers)
        .body(req_body.to_string())
        .send()
        .await?;

    // Connection data
    let body = res.text().await;
    if body.is_err() {
        return Err(ShopifyAPIError::ResponseBroken);
    }
    let body = body.unwrap();

    log::debug!(
        "shopify (url: {}) response: {body} \n With body: {}",
        shopify.get_query_url(),
        req_body.to_string()
    );

    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(ShopifyAPIError::JsonParseError)?;

    // Check if the query was THROTTLED
    if let Some(error) = json["errors"]["01"]["extensions"]["code"].as_str() {
        if error == "THROTTLED" {
            return Err(ShopifyAPIError::Throttled);
        }
    }

    let json = match read_json_tree(&json, json_finder) {
        Ok(v) => v,
        Err(_) => {
            return Err(ShopifyAPIError::NotWantedJsonFormat(json.to_string()));
        }
    };

    let end_json: ReturnType = match serde_json::from_value(json.to_owned()) {
        Ok(v) => v,
        Err(_) => {
            // The shopify response is not wanted json
            return Err(ShopifyAPIError::NotWantedJsonFormat(json.to_string()));
        }
    };

    Ok(end_json)
}

impl Shopify {
    /// Query graphql shopify api
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// use shopify_api::utils::ReadJsonTreeSteps;
    /// use serde::{Deserialize};
    ///
    /// #[derive(Deserialize)]
    /// struct Shop {
    ///    name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///   let shopify = Shopify::new(env!("TEST_SHOP_NAME"), env!("TEST_KEY"), ShopifyAPIVersion::V2023_01, None);
    ///   let graphql_query = r#"
    ///      query {
    ///         shop {
    ///          name
    ///         }
    ///     }
    ///   "#;
    ///   let variables = serde_json::json!({});
    ///   let json_finder = vec![ReadJsonTreeSteps::Key("data"), ReadJsonTreeSteps::Key("shop")];
    ///   let shop: Shop = shopify.graphql_query(graphql_query, &variables, &json_finder).await.unwrap();
    ///
    ///   assert_eq!(shop.name, "Rust api");
    /// }
    ///
    ///
    /// ```
    pub async fn graphql_query<ReturnType, VariablesType>(
        &self,
        graphql_query: &str,
        variables: &VariablesType,
        json_finder: &Vec<ReadJsonTreeSteps<'_>>,
    ) -> Result<ReturnType, ShopifyAPIError>
    where
        ReturnType: serde::de::DeserializeOwned,
        VariablesType: serde::Serialize,
    {
        let args = (self, graphql_query, variables, json_finder);
        let response_json = utils::retry_async(
            10,
            shopify_graphql_query::<VariablesType, ReturnType>,
            &args,
        )
        .await?;

        Ok(response_json)
    }
}
