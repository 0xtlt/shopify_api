# Changelog

- Add: `debug` feature added, enabled by default

## 0.8.7

- Fixed: `wait_for_bulk` method condition

## 0.8.5

- Fixed: `wait_for_bulk` method condition

## 0.8.4

- Fixed: `UnsignedInt64` types to String

## 0.8.3

- Fixed: `confirmation_number` in `order` struct

## 0.8.2

- Fixed: Webhook Customer Struct

## 0.7.0

- Add: `graphql_client` feature
- Add: `post_graphql` method to `Shopify` with `graphql_client` feature on
- Edited: `warp_wrapper` method takes one more argument

## 0.6.0

- Add: `verify_hmac` method added to `Shopify`
- Add: `ShopifyWebhook` struct
- Add: `list_webhooks` method added to `Shopify`
- Add: `add_webhook` method added to `Shopify`
- Add: `edit_webhook` method added to `Shopify`
- Add: `delete_webhook` method added to `Shopify`
- Add: `webhook_auto_config` method added to `Shopify`

## 0.5.0

- Add: `stage_upload_json` method added to `Shopify`
- Add: `generate_staged_upload_url` method added to `Shopify`
- Add: `stage_upload_prepare` method added to `Shopify`
- Removed: `ShopifyAPIVersion` enum replaced by a string

## 0.4.7

- Fixed: `get_end_of_support_date` enum by @davidhollenbeckx

## 0.4.5

- Add: `download_bulk` method added to `Shopify`

## 0.4.2

- Add: The body response (string) to `NotWantedJsonFormat` error

## 0.4.1

- Add: `wait_for_bulk` method added to `Shopify`

## 0.4.0

- Add: `make_bulk_query` method added to `Shopify`
- Add: `get_bulk_by_id` method added to `Shopify`

## 0.3.6

- Updated: `ShopifyAPIVersion` enum

## 0.3.5

- Updated: `ShopifyAPIVersion` enum to add the latest version `V2023_04`

## 0.3.0

- Add the support of the REST API with the `rest_query` method added to `Shopify`
- Add `get_api_endpoint` method to `Shopify`
- Add `rest_url` method to `Shopify`
- Fix a bug for `retry_async` and `retry_sync` functions which run 2 times at least the given function.

## 0.2.1

- Add Changelog and readme
