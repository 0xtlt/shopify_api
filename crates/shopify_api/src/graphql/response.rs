//! What a GraphQL answer holds besides `data`: errors, user errors, cost.

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An entry of the `errors` of an answer.
///
/// Its `Debug` shows `message`, `locations`, `path` and `extensions.code`,
/// and how many other entries `extensions` has, not their names or values:
/// Shopify puts the whole value of a variable that does not validate in
/// `extensions.value`, a signed address included. The fields themselves stay
/// available.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GraphqlError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<Location>>,
    /// The path of the field that failed, names and indexes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

impl fmt::Debug for GraphqlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraphqlError")
            .field("message", &self.message)
            .field("locations", &self.locations)
            .field("path", &self.path)
            .field("extensions", &self.extensions.as_ref().map(ExtensionsShape))
            .finish()
    }
}

/// `extensions.code`, and how many other entries there are.
struct ExtensionsShape<'a>(&'a Value);

impl fmt::Debug for ExtensionsShape<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Value::Object(entries) = self.0 else {
            return formatter.write_str("..");
        };
        let code = entries.get("code").and_then(Value::as_str);
        let others = entries.len() - usize::from(entries.contains_key("code"));
        let mut shown = formatter.debug_struct("Extensions");
        if let Some(code) = code {
            shown.field("code", &code);
        }
        shown
            .field("others", &format_args!("{others} entries"))
            .finish()
    }
}

impl GraphqlError {
    /// `extensions.code`, such as `THROTTLED`, `ACCESS_DENIED` or `MAX_COST_EXCEEDED`.
    pub fn code(&self) -> Option<&str> {
        self.extensions.as_ref()?.get("code")?.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Location {
    pub line: u32,
    pub column: u32,
}

/// An entry of the `userErrors` of a mutation: what Shopify refused, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UserError {
    /// The path of the input field at fault.
    #[serde(default)]
    pub field: Option<Vec<String>>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// The cost of a query and the state of the shop's rate limit, from
/// `extensions.cost`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Cost {
    pub requested_query_cost: f64,
    #[serde(default)]
    pub actual_query_cost: Option<f64>,
    pub throttle_status: ThrottleStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ThrottleStatus {
    /// The size of the bucket.
    pub maximum_available: f64,
    /// What is left in it.
    pub currently_available: f64,
    /// What comes back each second.
    pub restore_rate: f64,
}

impl Cost {
    /// How long until the bucket holds the requested cost again, in seconds.
    pub fn seconds_until_available(&self) -> f64 {
        let missing = self.requested_query_cost - self.throttle_status.currently_available;
        if missing <= 0.0 || self.throttle_status.restore_rate <= 0.0 {
            0.0
        } else {
            missing / self.throttle_status.restore_rate
        }
    }
}

fn cost_of(extensions: Option<&Value>) -> Option<Cost> {
    serde_json::from_value(extensions?.get("cost")?.clone()).ok()
}

/// The typed `data` of an operation, with the `extensions` of the answer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Response<D> {
    pub data: D,
    pub extensions: Option<Value>,
}

impl<D> Response<D> {
    /// What the operation cost, and what is left of the rate limit.
    pub fn cost(&self) -> Option<Cost> {
        cost_of(self.extensions.as_ref())
    }
}

/// A GraphQL answer as Shopify sent it: `data`, `errors` and `extensions`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RawResponse {
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default, deserialize_with = "errors")]
    pub errors: Vec<GraphqlError>,
    #[serde(default)]
    pub extensions: Option<Value>,
}

/// `errors` is a list of errors; Shopify answers a few refusals with a
/// string instead (`"errors": "Not Found"`). Anything else is one error whose
/// `extensions.value` holds it.
fn errors<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<GraphqlError>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Errors {
        List(Vec<GraphqlError>),
        Message(String),
        Other(Value),
    }
    Ok(match Option::<Errors>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(Errors::List(errors)) => errors,
        Some(Errors::Message(message)) => vec![GraphqlError {
            message,
            locations: None,
            path: None,
            extensions: None,
        }],
        // Kept in `extensions.value`, out of the message an error displays.
        Some(Errors::Other(value)) => vec![GraphqlError {
            message: "Shopify answered `errors` that are neither a list nor a string".to_string(),
            locations: None,
            path: None,
            extensions: Some(serde_json::json!({ "value": value })),
        }],
    })
}

impl RawResponse {
    pub fn cost(&self) -> Option<Cost> {
        cost_of(self.extensions.as_ref())
    }

    /// Shopify did not run the operation: the rate limit of the shop is reached.
    pub fn is_throttled(&self) -> bool {
        self.errors
            .iter()
            .any(|error| error.code() == Some("THROTTLED"))
    }

    /// The `data` read as `T`, the path of the first mismatch in the error.
    pub fn data_as<T: DeserializeOwned>(&self) -> crate::Result<T> {
        crate::decode::from_value(self.data.clone().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_errors_cost_and_throttling() {
        let raw: RawResponse = serde_json::from_value(json!({
            "errors": [{ "message": "Throttled", "extensions": { "code": "THROTTLED" } }],
            "extensions": { "cost": {
                "requestedQueryCost": 752,
                "actualQueryCost": null,
                "throttleStatus": { "maximumAvailable": 2000.0, "currentlyAvailable": 252, "restoreRate": 100.0 },
            } },
        }))
        .unwrap();
        assert!(raw.is_throttled());
        assert_eq!(raw.errors[0].code(), Some("THROTTLED"));
        let cost = raw.cost().unwrap();
        assert_eq!(cost.requested_query_cost, 752.0);
        assert_eq!(cost.seconds_until_available(), 5.0);
    }

    #[test]
    fn a_string_of_errors_is_one_error() {
        let raw: RawResponse = serde_json::from_str(r#"{"errors":"Not Found"}"#).unwrap();
        assert_eq!(raw.errors.len(), 1);
        assert_eq!(raw.errors[0].message, "Not Found");
        let raw: RawResponse =
            serde_json::from_str(r#"{"data":{"shop":{"name":"x"}},"errors":null}"#).unwrap();
        assert!(raw.errors.is_empty());
    }

    #[test]
    fn errors_of_another_shape_keep_their_value_out_of_the_message() {
        let raw: RawResponse =
            serde_json::from_str(r#"{"errors":{"token":"shpat_SECRET"}}"#).unwrap();
        assert_eq!(raw.errors.len(), 1);
        let error = &raw.errors[0];
        assert!(!error.message.contains("SECRET"), "{}", error.message);
        assert!(!format!("{error:?}").contains("SECRET"), "{error:?}");
        assert_eq!(
            error.extensions.as_ref().unwrap()["value"]["token"],
            "shpat_SECRET"
        );
    }

    #[test]
    fn decoding_names_the_path_of_the_mismatch() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code, reason = "only decoded: the test checks the decoding")]
        struct Data {
            orders: Vec<Order>,
        }
        #[derive(Debug, Deserialize)]
        #[allow(dead_code, reason = "only decoded: the test checks the decoding")]
        struct Order {
            name: String,
        }
        let error =
            crate::decode::from_value::<Data>(json!({ "orders": [{ "name": "#1" }, { "id": 2 }] }))
                .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Shopify's answer does not match the operation at `orders[1]`: missing field `name`"
        );
    }
}
