use super::WebhookSubscription;
use crate::{Shopify, ShopifyAPIError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct WebhookSubscriptionsData {
    #[serde(rename = "webhookSubscriptions")]
    webhook_subscriptions: WebhookSubscriptionConnection,
}

#[derive(Debug, Deserialize)]
struct WebhookSubscriptionConnection {
    edges: Vec<WebhookSubscriptionEdge>,
}

#[derive(Debug, Deserialize)]
struct WebhookSubscriptionEdge {
    node: WebhookSubscriptionNode,
}

#[derive(Debug, Deserialize)]
struct WebhookSubscriptionNode {
    id: String,
    topic: String,
    format: String,
    uri: String,
}

#[derive(Debug, Deserialize)]
struct WebhookCreateData {
    #[serde(rename = "webhookSubscriptionCreate")]
    webhook_subscription_create: WebhookMutationPayload,
}

#[derive(Debug, Deserialize)]
struct WebhookDeleteData {
    #[serde(rename = "webhookSubscriptionDelete")]
    webhook_subscription_delete: WebhookDeletePayload,
}

#[derive(Debug, Deserialize)]
struct WebhookMutationPayload {
    #[serde(rename = "webhookSubscription")]
    webhook_subscription: Option<WebhookSubscriptionNode>,
    #[serde(rename = "userErrors")]
    user_errors: Vec<WebhookUserError>,
}

#[derive(Debug, Deserialize)]
struct WebhookDeletePayload {
    #[serde(rename = "deletedWebhookSubscriptionId")]
    deleted_webhook_subscription_id: Option<String>,
    #[serde(rename = "userErrors")]
    user_errors: Vec<WebhookUserError>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct WebhookUserError {
    field: Option<Vec<String>>,
    message: String,
}

impl From<WebhookSubscriptionNode> for WebhookSubscription {
    fn from(node: WebhookSubscriptionNode) -> Self {
        Self {
            id: node.id,
            topic: node.topic,
            format: node.format,
            uri: node.uri,
        }
    }
}

impl Shopify {
    pub async fn list_webhooks(&self) -> Result<Vec<WebhookSubscription>, ShopifyAPIError> {
        let data: WebhookSubscriptionsData = self
            .graphql(
                r#"
                query webhookSubscriptions {
                    webhookSubscriptions(first: 250) {
                        edges {
                            node {
                                id
                                topic
                                format
                                uri
                            }
                        }
                    }
                }
                "#,
                &json!({}),
            )
            .await?;

        Ok(data
            .webhook_subscriptions
            .edges
            .into_iter()
            .map(|edge| edge.node.into())
            .collect())
    }

    pub async fn add_webhook(
        &self,
        address: &str,
        topic: &str,
        format: &str,
    ) -> Result<WebhookSubscription, ShopifyAPIError> {
        let data: WebhookCreateData = self
            .graphql(
                r#"
                mutation webhookSubscriptionCreate(
                    $topic: WebhookSubscriptionTopic!,
                    $uri: String!,
                    $format: WebhookSubscriptionFormat!
                ) {
                    webhookSubscriptionCreate(
                        topic: $topic,
                        webhookSubscription: {
                            uri: $uri,
                            format: $format
                        }
                    ) {
                        webhookSubscription {
                            id
                            topic
                            format
                            uri
                        }
                        userErrors {
                            field
                            message
                        }
                    }
                }
                "#,
                &json!({
                    "topic": topic,
                    "uri": address,
                    "format": format,
                }),
            )
            .await?;

        if !data.webhook_subscription_create.user_errors.is_empty() {
            return Err(ShopifyAPIError::Other(format!(
                "webhookSubscriptionCreate returned errors: {:?}",
                data.webhook_subscription_create.user_errors
            )));
        }

        data.webhook_subscription_create
            .webhook_subscription
            .map(Into::into)
            .ok_or_else(|| ShopifyAPIError::Other("no webhook subscription returned".to_string()))
    }

    pub async fn delete_webhook(&self, webhook_id: &str) -> Result<(), ShopifyAPIError> {
        let data: WebhookDeleteData = self
            .graphql(
                r#"
                mutation webhookSubscriptionDelete($id: ID!) {
                    webhookSubscriptionDelete(id: $id) {
                        deletedWebhookSubscriptionId
                        userErrors {
                            field
                            message
                        }
                    }
                }
                "#,
                &json!({ "id": webhook_id }),
            )
            .await?;

        if !data.webhook_subscription_delete.user_errors.is_empty() {
            return Err(ShopifyAPIError::Other(format!(
                "webhookSubscriptionDelete returned errors: {:?}",
                data.webhook_subscription_delete.user_errors
            )));
        }

        data.webhook_subscription_delete
            .deleted_webhook_subscription_id
            .map(|_| ())
            .ok_or_else(|| ShopifyAPIError::Other("webhook was not deleted".to_string()))
    }

    pub async fn webhook_auto_config(
        &self,
        desired_webhooks: Vec<(&str, &str, &str)>,
    ) -> Result<(), ShopifyAPIError> {
        let existing_webhooks = self.list_webhooks().await?;
        let mut desired = desired_webhooks;

        for webhook in &existing_webhooks {
            let matches_desired = desired.iter().position(|(address, topic, format)| {
                webhook.uri == *address && webhook.topic == *topic && webhook.format == *format
            });

            if let Some(index) = matches_desired {
                desired.remove(index);
            } else {
                self.delete_webhook(&webhook.id).await?;
            }
        }

        for (address, topic, format) in desired {
            self.add_webhook(address, topic, format).await?;
        }

        Ok(())
    }
}
