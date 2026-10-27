//! Cursor pagination with typed operations.
#![cfg(feature = "testing")]

use serde_json::json;
use shopify_api::testing::{MockTransport, client};
use shopify_api::{Page, graphql};

#[tokio::test]
async fn collects_every_page_following_the_cursor() {
    let mock = MockTransport::new();
    for (ids, next, cursor) in [
        (vec![1, 2], true, Some("c2")),
        (vec![3, 4], true, Some("c4")),
        (vec![5], false, None),
    ] {
        let nodes: Vec<_> = ids
            .iter()
            .map(|id| json!({ "id": format!("gid://shopify/Product/{id}"), "title": format!("P{id}") }))
            .collect();
        mock.push_data(json!({ "products": {
            "nodes": nodes,
            "pageInfo": { "hasNextPage": next, "endCursor": cursor },
        } }));
    }
    let shopify = client(&mock);

    let products = shopify
        .paginate(
            |after| {
                graphql!(
                    "query Products($after: String) {
                       products(first: 2, after: $after) {
                         nodes { id title }
                         pageInfo { hasNextPage endCursor }
                       }
                     }",
                    after = after,
                )
            },
            |data| {
                let page_info = data.products.page_info;
                Page::new(
                    data.products.nodes,
                    page_info.has_next_page,
                    page_info.end_cursor,
                )
            },
        )
        .await
        .unwrap();

    let titles: Vec<&str> = products
        .iter()
        .map(|product| product.title.as_str())
        .collect();
    assert_eq!(titles, ["P1", "P2", "P3", "P4", "P5"]);
    let cursors: Vec<_> = mock
        .graphql_requests()
        .into_iter()
        .map(|request| request.variables.get("after").cloned())
        .collect();
    assert_eq!(
        cursors,
        [Some(json!(null)), Some(json!("c2")), Some(json!("c4"))]
    );
}

#[tokio::test]
async fn a_cursor_that_does_not_move_stops_the_loop() {
    let mock = MockTransport::new();
    mock.respond_with(|_| {
        Ok(shopify_api::transport::HttpResponse::json(
            200,
            &json!({ "data": { "products": { "nodes": [], "pageInfo": { "hasNextPage": true, "endCursor": "same" } } } }),
        ))
    });
    let shopify = client(&mock);

    let error = shopify
        .paginate(
            |after| {
                graphql!(
                    "query($after: String) { products(first: 2, after: $after) { nodes { id } pageInfo { hasNextPage endCursor } } }",
                    after = after,
                )
            },
            |data| {
                let page_info = data.products.page_info;
                Page::new(data.products.nodes, page_info.has_next_page, page_info.end_cursor)
            },
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("the cursor does not move"),
        "{error}"
    );
    assert_eq!(mock.requests().len(), 2);
}
