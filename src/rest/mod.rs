use std::collections::HashMap;

use crate::{
    utils::{self, ReadJsonTreeSteps},
    Shopify, ShopifyAPIError,
};

pub enum ShopifyAPIRestType<'a> {
    Get(&'a str, &'a HashMap<&'a str, &'a str>),
    Post(
        &'a str,
        &'a HashMap<&'a str, &'a str>,
        &'a serde_json::Value,
    ),
    Put(
        &'a str,
        &'a HashMap<&'a str, &'a str>,
        &'a serde_json::Value,
    ),
    Delete(&'a str, &'a HashMap<&'a str, &'a str>),
}

async fn shopify_rest_query<ReturnType>(
    (shopify, endpoint, json_finder): &(
        &Shopify,
        &ShopifyAPIRestType<'_>,
        &Option<Vec<ReadJsonTreeSteps<'_>>>,
    ),
) -> Result<ReturnType, ShopifyAPIError>
where
    ReturnType: serde::de::DeserializeOwned,
{
    // Prepare the client
    let client = reqwest::Client::new();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("X-Shopify-Access-Token", shopify.api_key.parse().unwrap());

    let req = match endpoint {
        ShopifyAPIRestType::Get(url, params) => client
            .get(shopify.get_api_endpoint(url))
            .headers(headers)
            .query(params),

        ShopifyAPIRestType::Post(url, params, body) => client
            .post(shopify.get_api_endpoint(url))
            .headers(headers)
            .query(params)
            .body(body.to_string()),

        ShopifyAPIRestType::Put(url, params, body) => client
            .put(shopify.get_api_endpoint(url))
            .headers(headers)
            .query(params)
            .body(body.to_string()),

        ShopifyAPIRestType::Delete(url, params) => client
            .delete(shopify.get_api_endpoint(url))
            .headers(headers)
            .query(params),
    };

    // Connection Response
    let res = match req.send().await {
        Ok(local_response) => local_response,
        Err(_) => {
            return Err(ShopifyAPIError::ConnectionFailed);
        }
    };

    // Connection data
    let body = res.text().await;
    if body.is_err() {
        return Err(ShopifyAPIError::ResponseBroken);
    }

    let body = body.unwrap();

    let json: serde_json::Value = {
        match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => {
                // The shopify response is not valid json
                return Err(ShopifyAPIError::NotJson);
            }
        }
    };

    let json = match json_finder {
        Some(json_finder) => match utils::read_json_tree(&json, json_finder) {
            Ok(v) => v,
            Err(_) => {
                return Err(ShopifyAPIError::NotWantedJsonFormat);
            }
        },
        None => &json,
    };

    let json = match serde_json::to_string(json) {
        Ok(v) => v,
        Err(_) => {
            return Err(ShopifyAPIError::NotWantedJsonFormat);
        }
    };

    let json = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => {
            return Err(ShopifyAPIError::NotWantedJsonFormat);
        }
    };

    Ok(json)
}

impl Shopify {
    /// Query REST shopify api
    /// # Example
    /// ```
    /// use std::collections::HashMap;
    /// use shopify_api::*;
    /// use shopify_api::utils::ReadJsonTreeSteps;
    /// use shopify_api::rest::ShopifyAPIRestType;
    /// use serde::{Deserialize};
    ///
    /// #[derive(Deserialize, Debug)]
    /// struct Product {
    ///    id: u64,
    ///    title: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///    let shopify = Shopify::new(env!("TEST_SHOP_NAME"), env!("TEST_KEY"), ShopifyAPIVersion::V2023_01, None);
    ///   let json_finder = vec![ReadJsonTreeSteps::Key("products"), ReadJsonTreeSteps::Index(0)];
    ///
    ///  let product: Product = shopify.rest_query(&ShopifyAPIRestType::Get("products.json", &HashMap::new()), &Some(json_finder)).await.unwrap();
    ///
    /// assert_eq!(product.title, String::from("Hello world product"));
    /// }
    ///```
    pub async fn rest_query<ReturnType>(
        &self,
        rest_query: &ShopifyAPIRestType<'_>,
        json_finder: &Option<Vec<ReadJsonTreeSteps<'_>>>,
    ) -> Result<ReturnType, ShopifyAPIError>
    where
        ReturnType: serde::de::DeserializeOwned,
    {
        let args = (self, rest_query, json_finder);
        let response_json = utils::retry_async(10, shopify_rest_query::<ReturnType>, &args).await?;

        Ok(response_json)
    }
}
