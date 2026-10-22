//! Webhooks: verify and read what Shopify sends, and manage the webhook
//! subscriptions of a shop.
//!
//! ```
//! use shopify_api::webhooks::Webhook;
//!
//! # fn handle(headers: &[(&str, &str)], body: &[u8]) -> shopify_api::Result<()> {
//! // In your HTTP handler, with the raw body, before parsing it.
//! let webhook = Webhook::verify(headers.iter().copied(), body, "the app's client secret")?;
//! match webhook.topic() {
//!     "orders/create" => {
//!         let order: serde_json::Value = webhook.payload()?;
//!         println!("order {} on {}", order["name"], webhook.shop());
//!     }
//!     other => println!("{other} ignored"),
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Shopify may deliver a webhook more than once: [`Webhook::event_id`]
//! identifies the event across deliveries.
//!
//! The signature covers the body only. The headers (the topic, the shop, the
//! IDs) are what Shopify sent along, unsigned: a captured body could be
//! replayed with other headers. Check what matters against the body, or
//! against your own records of the shop.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::{Error, Result, ShopDomain, Shopify, UserError};
use shopify_api_macros::__graphql_internal as graphql;

/// Whether `hmac_header` (`X-Shopify-Hmac-Sha256`) is the signature of
/// `body` with `secret`, the client secret of the app. Compares in constant time.
pub fn verify_hmac(secret: &str, body: &[u8], hmac_header: &str) -> bool {
    // Anyone can sign with an empty key: it verifies nothing.
    if secret.is_empty() {
        return false;
    }
    let Ok(expected) = STANDARD.decode(hmac_header.trim()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// The signature Shopify would send for `body`, base64: for tests.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(body);
    STANDARD.encode(mac.finalize().into_bytes())
}

/// A webhook whose signature was verified.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Webhook {
    topic: String,
    shop: ShopDomain,
    webhook_id: Option<String>,
    event_id: Option<String>,
    api_version: Option<String>,
    triggered_at: Option<String>,
    sub_topic: Option<String>,
    name: Option<String>,
    body: Vec<u8>,
}

impl Webhook {
    /// Verifies a webhook from its headers and raw body, and reads its headers.
    ///
    /// `headers` are name and value pairs, the names in any case. Fails when
    /// the signature is missing or does not match, or a header Shopify always
    /// sends is missing.
    pub fn verify<K, V>(
        headers: impl IntoIterator<Item = (K, V)>,
        body: impl Into<Vec<u8>>,
        secret: &str,
    ) -> Result<Self>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self::verify_with_secrets(headers, body, &[secret])
    }

    /// [`Webhook::verify`], accepting the signature of any of `secrets`:
    /// while the client secret of the app is being rotated.
    pub fn verify_with_secrets<K, V>(
        headers: impl IntoIterator<Item = (K, V)>,
        body: impl Into<Vec<u8>>,
        secrets: &[&str],
    ) -> Result<Self>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let headers: Vec<(String, String)> = headers
            .into_iter()
            .map(|(name, value)| {
                (
                    name.as_ref().to_ascii_lowercase(),
                    value.as_ref().to_string(),
                )
            })
            .collect();
        let header = |name: &str| {
            headers
                .iter()
                .find(|(given, _)| given == name)
                .map(|(_, value)| value.trim().to_string())
        };
        let required = |name: &'static str| {
            header(name)
                .ok_or_else(|| Error::Verification(format!("the webhook has no `{name}` header")))
        };

        let body = body.into();
        let signature = required("x-shopify-hmac-sha256")?;
        if secrets.iter().all(|secret| secret.is_empty()) {
            return Err(Error::Verification(
                "no secret to verify the webhook with: the client secret of the app is empty"
                    .to_string(),
            ));
        }
        if !secrets
            .iter()
            .any(|secret| verify_hmac(secret, &body, &signature))
        {
            return Err(Error::Verification(
                "the signature of the webhook does not match".to_string(),
            ));
        }
        let shop = ShopDomain::new(&required("x-shopify-shop-domain")?).map_err(|error| {
            Error::Verification(format!("the webhook names an invalid shop: {error}"))
        })?;
        Ok(Self {
            topic: required("x-shopify-topic")?,
            shop,
            webhook_id: header("x-shopify-webhook-id"),
            event_id: header("x-shopify-event-id"),
            api_version: header("x-shopify-api-version"),
            triggered_at: header("x-shopify-triggered-at"),
            sub_topic: header("x-shopify-sub-topic"),
            name: header("x-shopify-name"),
            body,
        })
    }

    /// `orders/create`.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// The topic as the Admin API writes it: `ORDERS_CREATE`.
    pub fn topic_enum(&self) -> String {
        topic_enum(&self.topic)
    }

    pub fn shop(&self) -> &ShopDomain {
        &self.shop
    }

    /// Identifies this delivery.
    pub fn webhook_id(&self) -> Option<&str> {
        self.webhook_id.as_deref()
    }

    /// Identifies the event, the same across deliveries: to ignore a duplicate.
    pub fn event_id(&self) -> Option<&str> {
        self.event_id.as_deref()
    }

    /// The API version of the payload.
    pub fn api_version(&self) -> Option<&str> {
        self.api_version.as_deref()
    }

    /// When the event happened, as Shopify wrote it.
    pub fn triggered_at(&self) -> Option<&str> {
        self.triggered_at.as_deref()
    }

    pub fn sub_topic(&self) -> Option<&str> {
        self.sub_topic.as_deref()
    }

    /// The name of the subscription, when it has one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The raw body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body read as `T`, the path of the first mismatch in the error.
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T> {
        let mut deserializer = serde_json::Deserializer::from_slice(&self.body);
        serde_path_to_error::deserialize(&mut deserializer).map_err(|error| Error::Decode {
            path: error.path().to_string(),
            message: error.inner().to_string(),
        })
    }
}

/// `orders/create` → `ORDERS_CREATE`. A topic already in that form is kept.
pub fn topic_enum(topic: &str) -> String {
    topic
        .trim()
        .replace(['/', '.', '-'], "_")
        .to_ascii_uppercase()
}

// ─── Subscriptions ───────────────────────────────────────────────────

graphql! {
    mod list = file("graphql/operations/webhooks_list.graphql");
}

graphql! {
    mod create = file("graphql/operations/webhooks_create.graphql");
}

graphql! {
    mod update = file("graphql/operations/webhooks_update.graphql");
}

/// The input of `create` or `update` for a spec. An update sends every
/// option, empty when the spec has none, so that it replaces the old ones.
macro_rules! input {
    ($module:ident, $spec:expr, $replace:expr) => {{
        let spec: &WebhookSubscriptionSpec = $spec;
        let replace: bool = $replace;
        let list = |values: &Vec<String>| (replace || !values.is_empty()).then(|| values.clone());
        $module::WebhookSubscriptionInput {
            uri: Some(spec.uri.clone()),
            format: Some(spec.format.clone().unwrap_or_else(|| "JSON".to_string())),
            filter: spec.filter.clone().or_else(|| replace.then(String::new)),
            include_fields: list(&spec.include_fields),
            metafield_namespaces: list(&spec.metafield_namespaces),
            name: spec.name.clone(),
            ..Default::default()
        }
    }};
}

graphql! {
    mod delete = file("graphql/operations/webhooks_delete.graphql");
}

/// A webhook subscription of a shop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WebhookSubscription {
    pub id: String,
    /// `ORDERS_CREATE`.
    pub topic: String,
    /// An HTTPS address, a `pubsub://` or an Amazon EventBridge `arn:`.
    pub uri: String,
    /// `JSON` or `XML`.
    pub format: String,
    pub filter: Option<String>,
    pub include_fields: Vec<String>,
    pub metafield_namespaces: Vec<String>,
    pub name: Option<String>,
    /// The API version of the payloads.
    pub api_version: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    id: String,
    topic: String,
    uri: String,
    format: String,
    filter: Option<String>,
    include_fields: Vec<String>,
    metafield_namespaces: Vec<String>,
    name: Option<String>,
    created_at: String,
    updated_at: String,
    api_version: WireVersion,
}

#[derive(Deserialize)]
struct WireVersion {
    handle: String,
}

fn subscription_of(selection: &impl Serialize) -> Result<WebhookSubscription> {
    let wire: Wire = crate::graphql::decode(serde_json::to_value(selection)?)?;
    Ok(WebhookSubscription {
        id: wire.id,
        topic: wire.topic,
        uri: wire.uri,
        format: wire.format,
        filter: wire.filter,
        include_fields: wire.include_fields,
        metafield_namespaces: wire.metafield_namespaces,
        name: wire.name,
        api_version: wire.api_version.handle,
        created_at: wire.created_at,
        updated_at: wire.updated_at,
    })
}

fn refused(mutation: &'static str, errors: &impl Serialize) -> Result<()> {
    let errors: Vec<UserError> = crate::graphql::decode(serde_json::to_value(errors)?)?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::UserErrors { mutation, errors })
    }
}

/// A webhook subscription to create.
///
/// ```
/// use shopify_api::webhooks::WebhookSubscriptionSpec;
///
/// let spec = WebhookSubscriptionSpec::new("orders/create", "https://example.com/webhooks")
///     .include_fields(["id", "name", "total_price"])
///     .filter("total_price:>100");
/// assert_eq!(spec.topic(), "ORDERS_CREATE");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSubscriptionSpec {
    topic: String,
    uri: String,
    format: Option<String>,
    filter: Option<String>,
    include_fields: Vec<String>,
    metafield_namespaces: Vec<String>,
    name: Option<String>,
}

impl WebhookSubscriptionSpec {
    /// `topic` as `orders/create` or `ORDERS_CREATE`; `uri` an HTTPS address,
    /// a `pubsub://project:topic` or an Amazon EventBridge `arn:`.
    pub fn new(topic: impl AsRef<str>, uri: impl Into<String>) -> Self {
        Self {
            topic: topic_enum(topic.as_ref()),
            uri: uri.into(),
            format: None,
            filter: None,
            include_fields: Vec::new(),
            metafield_namespaces: Vec::new(),
            name: None,
        }
    }

    /// `JSON` (the default) or `XML`.
    pub fn format(mut self, format: impl AsRef<str>) -> Self {
        self.format = Some(format.as_ref().to_ascii_uppercase());
        self
    }

    /// Only the events that match this search.
    pub fn filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    /// Only these fields of the payload.
    pub fn include_fields<I: IntoIterator<Item = S>, S: Into<String>>(mut self, fields: I) -> Self {
        self.include_fields = fields.into_iter().map(Into::into).collect();
        self
    }

    /// The metafields of these namespaces in the payload.
    pub fn metafield_namespaces<I: IntoIterator<Item = S>, S: Into<String>>(
        mut self,
        namespaces: I,
    ) -> Self {
        self.metafield_namespaces = namespaces.into_iter().map(Into::into).collect();
        self
    }

    /// A name, sent in the `X-Shopify-Name` header of each delivery.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Whether `subscription` is this one: same topic, address and options.
    pub fn matches(&self, subscription: &WebhookSubscription) -> bool {
        let sorted = |values: &[String]| {
            let mut values = values.to_vec();
            values.sort();
            values
        };
        subscription.topic == self.topic
            && subscription.uri == self.uri
            && subscription.format == self.format.as_deref().unwrap_or("JSON")
            && subscription
                .filter
                .as_deref()
                .filter(|filter| !filter.is_empty())
                == self.filter.as_deref()
            && sorted(&subscription.include_fields) == sorted(&self.include_fields)
            && sorted(&subscription.metafield_namespaces) == sorted(&self.metafield_namespaces)
            && (self.name.is_none() || subscription.name == self.name)
    }
}

/// What [`Shopify::sync_webhook_subscriptions`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SyncReport {
    pub created: Vec<WebhookSubscription>,
    /// Subscriptions of a desired topic and address whose options changed.
    pub updated: Vec<WebhookSubscription>,
    /// The IDs of the subscriptions deleted.
    pub deleted: Vec<String>,
    pub kept: Vec<WebhookSubscription>,
}

impl Shopify {
    /// Every webhook subscription the app has on the shop. Those of the
    /// app's configuration file (`shopify.app.toml`) are not listed by
    /// Shopify.
    pub async fn webhook_subscriptions(&self) -> Result<Vec<WebhookSubscription>> {
        let pages = self
            .paginate(
                |after| list::operation(list::Variables { after }),
                |data| {
                    let connection = data.webhook_subscriptions;
                    crate::Page::new(
                        connection.nodes,
                        connection.page_info.has_next_page,
                        connection.page_info.end_cursor,
                    )
                },
            )
            .await?;
        pages.iter().map(subscription_of).collect()
    }

    pub async fn create_webhook_subscription(
        &self,
        spec: &WebhookSubscriptionSpec,
    ) -> Result<WebhookSubscription> {
        let input = input!(create, spec, false);
        let data = self
            .execute(create::operation(create::Variables {
                topic: spec.topic.clone(),
                webhook_subscription: input,
            }))
            .await?;
        let payload = data
            .webhook_subscription_create
            .ok_or_else(|| Error::Decode {
                path: "webhookSubscriptionCreate".to_string(),
                message: "Shopify returned nothing".to_string(),
            })?;
        refused("webhookSubscriptionCreate", &payload.user_errors)?;
        let subscription = payload.webhook_subscription.ok_or_else(|| Error::Decode {
            path: "webhookSubscriptionCreate.webhookSubscription".to_string(),
            message: "Shopify returned no subscription".to_string(),
        })?;
        subscription_of(&subscription)
    }

    pub async fn delete_webhook_subscription(&self, id: &str) -> Result<()> {
        let data = self
            .execute(delete::operation(delete::Variables { id: id.to_string() }))
            .await?;
        let payload = data
            .webhook_subscription_delete
            .ok_or_else(|| Error::Decode {
                path: "webhookSubscriptionDelete".to_string(),
                message: "Shopify returned nothing".to_string(),
            })?;
        refused("webhookSubscriptionDelete", &payload.user_errors)
    }

    /// Replaces the options of a subscription with those of `spec`; its
    /// topic stays.
    pub async fn update_webhook_subscription(
        &self,
        id: &str,
        spec: &WebhookSubscriptionSpec,
    ) -> Result<WebhookSubscription> {
        let data = self
            .execute(update::operation(update::Variables {
                id: id.to_string(),
                webhook_subscription: input!(update, spec, true),
            }))
            .await?;
        let payload = data
            .webhook_subscription_update
            .ok_or_else(|| Error::Decode {
                path: "webhookSubscriptionUpdate".to_string(),
                message: "Shopify returned nothing".to_string(),
            })?;
        refused("webhookSubscriptionUpdate", &payload.user_errors)?;
        let subscription = payload.webhook_subscription.ok_or_else(|| Error::Decode {
            path: "webhookSubscriptionUpdate.webhookSubscription".to_string(),
            message: "Shopify returned no subscription".to_string(),
        })?;
        subscription_of(&subscription)
    }

    /// Makes the shop's subscriptions exactly `desired`: those that match are
    /// kept, those of a desired topic and address are updated, the missing
    /// ones created, then the others deleted. Nothing is deleted before what
    /// replaces it exists, so no event is missed in between.
    ///
    /// It deletes every subscription of the app on the shop that `desired`
    /// does not list, including those another part of the app created.
    pub async fn sync_webhook_subscriptions(
        &self,
        desired: &[WebhookSubscriptionSpec],
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let mut missing: Vec<&WebhookSubscriptionSpec> = desired.iter().collect();
        let mut extra = Vec::new();
        for subscription in self.webhook_subscriptions().await? {
            match missing.iter().position(|spec| spec.matches(&subscription)) {
                Some(index) => {
                    missing.remove(index);
                    report.kept.push(subscription);
                }
                None => extra.push(subscription),
            }
        }
        // A desired topic and address that exists with other options.
        let mut leftover = Vec::new();
        for subscription in extra {
            let same_place = missing
                .iter()
                .position(|spec| spec.topic == subscription.topic && spec.uri == subscription.uri);
            match same_place {
                Some(index) => {
                    let spec = missing.remove(index);
                    report.updated.push(
                        self.update_webhook_subscription(&subscription.id, spec)
                            .await?,
                    );
                }
                None => leftover.push(subscription),
            }
        }
        for spec in missing {
            report
                .created
                .push(self.create_webhook_subscription(spec).await?);
        }
        for subscription in leftover {
            self.delete_webhook_subscription(&subscription.id).await?;
            report.deleted.push(subscription.id);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_the_signature_of_shopify() {
        let body = br#"{"id":1}"#;
        let signature = sign("secret", body);
        assert!(verify_hmac("secret", body, &signature));
        assert!(!verify_hmac("other", body, &signature));
        assert!(!verify_hmac("secret", b"{\"id\":2}", &signature));
        assert!(!verify_hmac("secret", body, "not base64!"));
        assert!(!verify_hmac("secret", body, ""));
    }

    fn headers(signature: &str) -> Vec<(&'static str, String)> {
        vec![
            ("X-Shopify-Topic", "orders/create".to_string()),
            ("X-Shopify-Hmac-SHA256", signature.to_string()),
            ("x-shopify-shop-domain", "my-shop.myshopify.com".to_string()),
            ("X-Shopify-Webhook-Id", "b54557e4".to_string()),
            ("X-Shopify-Event-Id", "98880550".to_string()),
            ("X-Shopify-API-Version", "2026-10".to_string()),
            ("X-Shopify-Triggered-At", "2026-10-22T10:00:00Z".to_string()),
        ]
    }

    #[test]
    fn reads_a_verified_webhook() {
        let body = br##"{"id":1,"name":"#1001"}"##.to_vec();
        let webhook =
            Webhook::verify(headers(&sign("secret", &body)), body.clone(), "secret").unwrap();
        assert_eq!(webhook.topic(), "orders/create");
        assert_eq!(webhook.topic_enum(), "ORDERS_CREATE");
        assert_eq!(webhook.shop().as_str(), "my-shop.myshopify.com");
        assert_eq!(webhook.webhook_id(), Some("b54557e4"));
        assert_eq!(webhook.event_id(), Some("98880550"));
        assert_eq!(webhook.api_version(), Some("2026-10"));
        assert_eq!(webhook.triggered_at(), Some("2026-10-22T10:00:00Z"));
        assert_eq!(webhook.sub_topic(), None);
        assert_eq!(webhook.body(), body);

        #[derive(Deserialize)]
        struct Order {
            name: String,
        }
        assert_eq!(webhook.payload::<Order>().unwrap().name, "#1001");
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Wrong {
            name: u32,
        }
        assert_eq!(
            webhook.payload::<Wrong>().unwrap_err().to_string(),
            "Shopify's answer does not match the operation at `name`: invalid type: string \"#1001\", expected u32 at line 1 column 22"
        );
    }

    #[test]
    fn refuses_what_does_not_verify() {
        let body = b"{}".to_vec();
        let error =
            Webhook::verify(headers(&sign("other", &body)), body.clone(), "secret").unwrap_err();
        assert_eq!(
            error.to_string(),
            "verification failed: the signature of the webhook does not match"
        );

        let unsigned: Vec<_> = headers("")
            .into_iter()
            .filter(|(name, _)| !name.contains("Hmac"))
            .collect();
        let error = Webhook::verify(unsigned, body.clone(), "secret").unwrap_err();
        assert_eq!(
            error.to_string(),
            "verification failed: the webhook has no `x-shopify-hmac-sha256` header"
        );

        // An empty secret, as from an unset variable, verifies nothing.
        let error = Webhook::verify(headers(&sign("", &body)), body.clone(), "").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the client secret of the app is empty"),
            "{error}"
        );
        assert!(!verify_hmac("", &body, &sign("", &body)));

        // A rotated secret: either verifies.
        let webhook = Webhook::verify_with_secrets(
            headers(&sign("old", &body)),
            body.clone(),
            &["new", "old"],
        );
        assert!(webhook.is_ok());

        let mut wrong_shop = headers(&sign("secret", &body));
        wrong_shop[2].1 = "evil.example.com".to_string();
        assert!(Webhook::verify(wrong_shop, body, "secret").is_err());
    }

    #[test]
    fn specs_match_subscriptions_whatever_the_order_of_their_fields() {
        let spec = WebhookSubscriptionSpec::new("orders/create", "https://example.com/hooks")
            .include_fields(["b", "a"]);
        let mut subscription = WebhookSubscription {
            id: "gid://shopify/WebhookSubscription/1".to_string(),
            topic: "ORDERS_CREATE".to_string(),
            uri: "https://example.com/hooks".to_string(),
            format: "JSON".to_string(),
            filter: Some(String::new()),
            include_fields: vec!["a".to_string(), "b".to_string()],
            metafield_namespaces: Vec::new(),
            name: None,
            api_version: "2026-10".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(spec.matches(&subscription));
        subscription.uri = "https://example.com/other".to_string();
        assert!(!spec.matches(&subscription));
        assert_eq!(topic_enum("app/uninstalled"), "APP_UNINSTALLED");
        assert_eq!(topic_enum("ORDERS_PAID"), "ORDERS_PAID");
    }
}
