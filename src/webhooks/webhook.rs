use super::ShopifyWebhook;
use crate::{rest::ShopifyAPIRestType, utils::ReadJsonTreeSteps, Shopify, ShopifyAPIError};
use serde_json::json;
use std::collections::HashMap;

impl Shopify {
    pub async fn list_webhooks(&self) -> Result<Vec<ShopifyWebhook>, ShopifyAPIError> {
        self.rest_query::<Vec<ShopifyWebhook>>(
            &ShopifyAPIRestType::Get("webhooks.json", &HashMap::new()),
            &Some(vec![ReadJsonTreeSteps::Key("webhooks")]),
        )
        .await
    }
    pub async fn add_webhook(
        &self,
        address: &str,
        topic: &str,
        format: &str,
    ) -> Result<ShopifyWebhook, ShopifyAPIError> {
        let webhook_data = json!({
            "webhook": {
                "address": address,
                "topic": topic,
                "format": format
            }
        });

        self.rest_query::<ShopifyWebhook>(
            &ShopifyAPIRestType::Post("webhooks.json", &HashMap::new(), &webhook_data),
            &Some(vec![ReadJsonTreeSteps::Key("webhook")]),
        )
        .await
    }
    pub async fn edit_webhook(
        &self,
        webhook_id: u64,
        new_address: &str,
    ) -> Result<ShopifyWebhook, ShopifyAPIError> {
        let webhook_data = json!({
            "webhook": {
                "id": webhook_id,
                "address": new_address,
            }
        });

        let endpoint = format!("webhooks/{}.json", webhook_id);
        self.rest_query::<ShopifyWebhook>(
            &ShopifyAPIRestType::Put(&endpoint, &HashMap::new(), &webhook_data),
            &Some(vec![ReadJsonTreeSteps::Key("webhook")]),
        )
        .await
    }
    pub async fn delete_webhook(
        &self,
        webhook_id: u64,
    ) -> Result<serde_json::Value, ShopifyAPIError> {
        let endpoint = format!("webhooks/{}.json", webhook_id);
        self.rest_query::<serde_json::Value>(
            &ShopifyAPIRestType::Delete(&endpoint, &HashMap::new()),
            &None,
        )
        .await
    }
    pub async fn webhook_auto_config(
        &self,
        desired_webhooks: Vec<(&str, &str, &str)>,
        // api_version: &str,
    ) -> Result<(), ShopifyAPIError> {
        let existing_webhooks = self.list_webhooks().await?;

        log::debug!("Existing webhooks: {:?}", existing_webhooks);

        let mut webhooks_to_import: Vec<(&str, &str, &str)> = Vec::new();
        let mut webhooks_to_delete: Vec<u64> = Vec::new();
        let mut webhooks_treated: Vec<u64> = Vec::new();

        for (address, topic, format) in desired_webhooks {
            let mut webhook_found = false;
            for webhook in &existing_webhooks {
                if webhook.address == address && webhook.topic == topic && webhook.format == format
                {
                    webhook_found = true;
                    webhooks_treated.push(webhook.id);
                    break;
                }
            }

            if !webhook_found {
                webhooks_to_import.push((address, topic, format));
            }
        }

        for webhook in &existing_webhooks {
            if !webhooks_treated.contains(&webhook.id) {
                webhooks_to_delete.push(webhook.id);
            }
        }

        log::debug!("Webhooks to delete: {:?}", webhooks_to_delete);
        for webhook_id in webhooks_to_delete {
            self.delete_webhook(webhook_id).await?;
        }

        log::debug!("Webhooks to import: {:?}", webhooks_to_import);
        for (address, topic, format) in webhooks_to_import {
            self.add_webhook(address, topic, format).await?;
        }

        Ok(())
    }
}
