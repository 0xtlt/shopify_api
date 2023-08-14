# Changelog

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
