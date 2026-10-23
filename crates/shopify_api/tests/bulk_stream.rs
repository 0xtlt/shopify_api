//! Bulk results read as they download, and bulk variables uploaded from an iterator.
#![cfg(feature = "testing")]

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shopify_api::bulk::{BulkWaitOptions, parse_jsonl};
use shopify_api::testing::{MockTransport, client};
use shopify_api::transport::{
    BoxFuture, HttpRequest, HttpResponse, StreamingResponse, Transport, TransportError,
};
use shopify_api::{Error, Shopify};

const RESULTS_URL: &str =
    "https://storage.googleapis.com/bulk-operation-outputs/1.jsonl?X-Goog-Signature=SECRET";
const UPLOAD_URL: &str = "https://shopify-staged-uploads.storage.googleapis.com/";

fn operation(status: &str, url: Option<&str>) -> Value {
    json!({
        "id": "gid://shopify/BulkOperation/1",
        "type": "QUERY",
        "status": status,
        "errorCode": null,
        "createdAt": "2026-10-22T10:00:00Z",
        "completedAt": null,
        "objectCount": "3",
        "rootObjectCount": "2",
        "fileSize": null,
        "url": url,
        "partialDataUrl": null,
        "query": "{ products { edges { node { id title } } } }",
    })
}

fn fast() -> BulkWaitOptions {
    BulkWaitOptions::new().poll_interval(Duration::from_millis(1))
}

fn push_run_and_completed(mock: &MockTransport, url: Option<&str>) {
    mock.push_data(json!({ "bulkOperationRunQuery": { "bulkOperation": operation("CREATED", None), "userErrors": [] } }));
    mock.push_data(json!({ "bulkOperation": operation("COMPLETED", url) }));
}

#[derive(Debug, Deserialize, PartialEq)]
struct Product {
    id: String,
    title: String,
}

#[tokio::test]
async fn a_bulk_query_streams_its_rows_from_chunks_cut_anywhere() {
    let mock = MockTransport::new();
    push_run_and_completed(&mock, Some(RESULTS_URL));
    mock.push_chunks(
        200,
        [
            &b"{\"id\":\"gid://shopify/Product/1\",\"title\":\"Caf"[..],
            b"\xc3",
            b"\xa9\"}\r\n\r\n{\"id\":\"gid://shopify/Product/2\",",
            b"\"title\":\"Hat\"}\n{\"id\":\"gid://shopify/Product/3\",\"title\":\"Cap\"}",
        ],
    );
    let shopify = client(&mock);

    let mut rows = shopify
        .bulk_query_stream::<Product>("{ products { edges { node { id title } } } }", fast())
        .await
        .unwrap();
    let mut titles = Vec::new();
    while let Some(row) = rows.next_row().await {
        titles.push(row.unwrap().title);
    }
    assert_eq!(titles, ["Café", "Hat", "Cap"]);
    assert_eq!(rows.line(), 4);

    let sent = mock.requests();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].url, RESULTS_URL);
    assert_eq!(sent[2].header("X-Shopify-Access-Token"), None);
    assert_eq!(mock.graphql_requests()[0].variables["groupObjects"], false);
}

#[tokio::test]
async fn an_operation_without_results_streams_nothing() {
    let mock = MockTransport::new();
    push_run_and_completed(&mock, None);
    let shopify = client(&mock);

    let mut rows = shopify
        .bulk_query_stream::<Product>("{ products { edges { node { id title } } } }", fast())
        .await
        .unwrap();
    assert!(rows.next_row().await.is_none());
    assert_eq!(mock.requests().len(), 2, "nothing to download");
}

#[tokio::test]
async fn a_refused_download_keeps_the_first_mib_of_its_body_and_hides_the_signature() {
    let mock = MockTransport::new();
    let chunk = vec![b'x'; 256 * 1024];
    let read = Arc::new(Mutex::new(0));
    let counter = read.clone();
    mock.push_chunks(
        403,
        std::iter::repeat_n(chunk, 32).inspect(move |_| *counter.lock().unwrap() += 1),
    );
    let shopify = client(&mock);

    let error = shopify
        .download_bulk_results_stream::<Value>(RESULTS_URL)
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(403));
    assert_eq!(error.raw_http_body().unwrap().len(), 1024 * 1024);
    assert_eq!(*read.lock().unwrap(), 4);
    let shown = format!("{error}\n{error:?}");
    assert!(!shown.contains("SECRET"), "{shown}");
}

struct Buffered(Vec<u8>);

impl Transport for Buffered {
    fn send(&self, _: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
        let response = HttpResponse::new(200, self.0.clone());
        Box::pin(async move { Ok(response) })
    }
}

#[tokio::test]
async fn a_transport_that_only_sends_streams_through_the_default() {
    let body = b"{\"id\":\"1\",\"title\":\"A\"}\n{\"id\":\"2\",\"title\":\"B\"}\n".to_vec();
    let shopify = Shopify::builder("test-shop")
        .access_token("t")
        .transport(Buffered(body.clone()))
        .build()
        .unwrap();

    let mut rows = shopify
        .download_bulk_results_stream::<Product>(RESULTS_URL)
        .await
        .unwrap();
    assert_eq!(rows.next_row().await.unwrap().unwrap().title, "A");
    assert_eq!(rows.next_row().await.unwrap().unwrap().title, "B");
    assert!(rows.next_row().await.is_none());

    let collected: Vec<Product> = shopify.download_bulk_results(RESULTS_URL).await.unwrap();
    assert_eq!(collected, parse_jsonl::<Product>(&body).unwrap());
}

struct FailsAfter(Vec<Vec<u8>>);

impl futures_core::Stream for FailsAfter {
    type Item = Result<Vec<u8>, TransportError>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(if self.0.is_empty() {
            Err(TransportError::new("connection reset"))
        } else {
            Ok(self.0.remove(0))
        }))
    }
}

#[tokio::test]
async fn the_vec_methods_stop_at_the_first_error_as_before() {
    let mock = MockTransport::new();
    mock.push_streaming(StreamingResponse::new(
        200,
        FailsAfter(vec![b"{\"id\":\"1\",\"title\":\"A\"}\n".to_vec()]),
    ));
    let shopify = client(&mock);
    let error = shopify
        .download_bulk_results::<Product>(RESULTS_URL)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
    assert!(!error.to_string().contains("SECRET"));

    let body = b"{\"id\":\"1\",\"title\":\"A\"}\n{\"id\":2}\n{\"id\":\"3\",\"title\":\"C\"}\n";
    mock.push_chunks(200, [&body[..7], &body[7..]]);
    let error = shopify
        .download_bulk_results::<Product>(RESULTS_URL)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        parse_jsonl::<Product>(body).unwrap_err().to_string()
    );
}

#[tokio::test]
async fn a_grouped_bulk_query_asks_for_grouping_and_yields_each_parent_with_its_children() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "bulkOperationRunQuery": { "bulkOperation": operation("CREATED", None), "userErrors": [] } }));
    mock.push_data(json!({ "bulkOperation": operation("COMPLETED", Some(RESULTS_URL)) }));
    mock.push_chunks(
        200,
        [
            &b"{\"id\":\"P1\",\"title\":\"Shirt\"}\n{\"sku\":\"S\",\"__par"[..],
            b"entId\":\"P1\"}\n{\"sku\":\"M\",\"__parentId\":\"P1\"}\n{\"id\":\"P2\",\"title\":\"Hat\"}\n",
        ],
    );
    let shopify = client(&mock);

    #[derive(Debug, Deserialize)]
    struct Variant {
        sku: String,
    }
    let mut groups = shopify
        .bulk_query_grouped::<Product, Variant>(
            "{ products { edges { node { id title variants { edges { node { sku } } } } } } }",
            fast(),
        )
        .await
        .unwrap();
    let (shirt, variants) = groups.next_group().await.unwrap().unwrap();
    assert_eq!(shirt.title, "Shirt");
    assert_eq!(
        variants.iter().map(|v| v.sku.as_str()).collect::<Vec<_>>(),
        ["S", "M"]
    );
    let (hat, variants) = groups.next_group().await.unwrap().unwrap();
    assert_eq!(hat.title, "Hat");
    assert!(variants.is_empty());
    assert!(groups.next_group().await.is_none());

    assert_eq!(mock.graphql_requests()[0].variables["groupObjects"], true);
}

#[tokio::test]
async fn a_child_out_of_place_only_asks_for_grouping_when_it_may_be_off() {
    let body = &b"{\"id\":\"P1\",\"title\":\"A\"}\n{\"id\":\"P2\",\"title\":\"B\"}\n{\"__parentId\":\"P1\"}\n"[..];
    let mock = MockTransport::new();
    push_run_and_completed(&mock, Some(RESULTS_URL));
    mock.push_chunks(200, [body]);
    mock.push_chunks(200, [body]);
    let shopify = client(&mock);

    let mut groups = shopify
        .bulk_query_grouped::<Product, Value>("{ products { edges { node { id } } } }", fast())
        .await
        .unwrap();
    groups.next_group().await.unwrap().unwrap();
    let error = groups.next_group().await.unwrap().unwrap_err().to_string();
    assert!(
        error.ends_with("though the query ran with `groupObjects`"),
        "{error}"
    );

    let mut groups = shopify
        .download_bulk_results_grouped::<Product, Value>(RESULTS_URL)
        .await
        .unwrap();
    groups.next_group().await.unwrap().unwrap();
    let error = groups.next_group().await.unwrap().unwrap_err().to_string();
    assert!(
        error.ends_with("run the query with `groupObjects`"),
        "{error}"
    );
}

fn upload_mock() -> (MockTransport, Arc<Mutex<Vec<HttpRequest>>>) {
    let mock = MockTransport::new();
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let seen = uploads.clone();
    mock.respond_with(move |request| {
        if request.url == UPLOAD_URL {
            seen.lock().unwrap().push(request.clone());
            return Ok(HttpResponse::new(201, ""));
        }
        let name = request.json().unwrap()["operationName"].as_str().unwrap().to_string();
        let data = match name.as_str() {
            "StagedUploadsCreate" => json!({ "stagedUploadsCreate": {
                "stagedTargets": [{
                    "url": UPLOAD_URL,
                    "resourceUrl": null,
                    "parameters": [{ "name": "key", "value": "tmp/1/bulk/vars.jsonl" }],
                }],
                "userErrors": [],
            } }),
            "BulkOperationRunMutation" => {
                let mut started = operation("CREATED", None);
                started["type"] = json!("MUTATION");
                json!({ "bulkOperationRunMutation": { "bulkOperation": started, "userErrors": [] } })
            }
            other => panic!("unexpected operation {other}"),
        };
        Ok(HttpResponse::json(200, &json!({ "data": data })))
    });
    (mock, uploads)
}

fn uploaded_file(request: &HttpRequest) -> String {
    let body = String::from_utf8(request.body.clone()).unwrap();
    let start = body.find("Content-Type: text/jsonl\r\n\r\n").unwrap() + 28;
    let end = body.rfind("\r\n--").unwrap();
    body[start..end].to_string()
}

#[tokio::test]
async fn bulk_variables_upload_from_an_iterator_as_from_a_slice() {
    let (mock, uploads) = upload_mock();
    let shopify = client(&mock);
    let mutation = "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }";

    #[derive(Serialize)]
    struct Row {
        product: Value,
    }
    let taken = Arc::new(Mutex::new(0));
    let counter = taken.clone();
    let rows = (1..=1000).map(move |id| {
        *counter.lock().unwrap() += 1;
        Row {
            product: json!({ "id": format!("gid://shopify/Product/{id}"), "title": "é\n\"" }),
        }
    });
    let started = shopify
        .run_bulk_mutation_iter(mutation, rows)
        .await
        .unwrap();
    assert_eq!(started.kind, shopify_api::bulk::BulkOperationType::Mutation);
    assert_eq!(*taken.lock().unwrap(), 1000);

    let slice: Vec<Row> = (1..=1000)
        .map(|id| Row {
            product: json!({ "id": format!("gid://shopify/Product/{id}"), "title": "é\n\"" }),
        })
        .collect();
    shopify.run_bulk_mutation(mutation, &slice).await.unwrap();

    let uploads = uploads.lock().unwrap();
    assert_eq!(uploads.len(), 2);
    let from_iter = uploaded_file(&uploads[0]);
    assert_eq!(from_iter, uploaded_file(&uploads[1]));
    let lines: Vec<Value> = parse_jsonl(from_iter.as_bytes()).unwrap();
    assert_eq!(lines.len(), 1000);
    assert_eq!(lines[999]["product"]["id"], "gid://shopify/Product/1000");
    assert_eq!(from_iter.matches('\n').count(), 1000, "one line per item");

    let graphql = mock.graphql_requests();
    assert_eq!(
        graphql[1].variables["stagedUploadPath"],
        "tmp/1/bulk/vars.jsonl"
    );
}

#[tokio::test]
async fn an_item_that_does_not_serialize_stops_the_upload() {
    struct Leaky;
    impl Serialize for Leaky {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("cannot serialize shpat_SECRET"))
        }
    }
    let (mock, uploads) = upload_mock();
    let shopify = client(&mock);

    let error = shopify
        .stage_bulk_variables_iter([Leaky])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Variables { .. }), "{error:?}");
    assert!(!format!("{error}{error:?}").contains("SECRET"));
    assert!(uploads.lock().unwrap().is_empty(), "nothing uploaded");
}

#[cfg(feature = "rustls")]
mod reqwest_transport {
    use super::*;
    use shopify_api::transport::{Method, ReqwestTransport};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(socket: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
        }
    }

    async fn write_chunk(socket: &mut TcpStream, data: &[u8]) {
        let mut chunk = format!("{:x}\r\n", data.len()).into_bytes();
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(b"\r\n");
        socket.write_all(&chunk).await.unwrap();
        socket.flush().await.unwrap();
    }

    async fn next(response: &mut StreamingResponse) -> Option<Result<Vec<u8>, TransportError>> {
        std::future::poll_fn(|context| {
            futures_core::Stream::poll_next(response.body.as_mut(), context)
        })
        .await
    }

    const HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/jsonl\r\nTransfer-Encoding: chunked\r\n\r\n";

    #[tokio::test]
    async fn rows_arrive_before_the_rest_of_the_body_is_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(HEAD).await.unwrap();
            write_chunk(
                &mut socket,
                b"{\"id\":\"1\",\"title\":\"A\"}\n{\"id\":\"2\",",
            )
            .await;
            released.await.unwrap();
            write_chunk(&mut socket, b"\"title\":\"B\"}\n").await;
            socket.write_all(b"0\r\n\r\n").await.unwrap();
        });
        let shopify = Shopify::new("test-shop", shopify_api::Auth::access_token("t")).unwrap();

        let mut rows = shopify
            .download_bulk_results_stream::<Product>(&format!("http://{address}/r.jsonl?sig=x"))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(10), rows.next_row())
            .await
            .expect("the first row before the end of the body");
        assert_eq!(first.unwrap().unwrap().title, "A");
        release.send(()).unwrap();
        assert_eq!(rows.next_row().await.unwrap().unwrap().title, "B");
        assert!(rows.next_row().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn the_timeout_bounds_each_chunk_not_the_whole_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(HEAD).await.unwrap();
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                write_chunk(&mut socket, b"{}\n").await;
            }
            socket.write_all(b"0\r\n\r\n").await.unwrap();
            drop(socket);
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(HEAD).await.unwrap();
            write_chunk(&mut socket, b"{}\n").await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let transport = ReqwestTransport::new().unwrap();
        let request = |url: String| {
            let mut request = HttpRequest::new(Method::Get, url);
            request.timeout = Some(Duration::from_millis(400));
            request
        };

        let started = std::time::Instant::now();
        let mut response = transport
            .send_streaming(request(format!("http://{address}/slow")))
            .await
            .unwrap();
        let mut body = Vec::new();
        while let Some(chunk) = next(&mut response).await {
            body.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(body, b"{}\n".repeat(6));
        assert!(
            started.elapsed() > Duration::from_millis(400),
            "longer than the timeout in all"
        );

        let mut response = transport
            .send_streaming(request(format!("http://{address}/stalled")))
            .await
            .unwrap();
        assert_eq!(next(&mut response).await.unwrap().unwrap(), b"{}\n");
        let error = next(&mut response).await.unwrap().unwrap_err();
        assert!(error.is_timeout(), "{error}");
        assert!(
            next(&mut response).await.is_none(),
            "the stream ends after its error"
        );
        server.abort();
    }

    #[tokio::test]
    async fn the_timeout_bounds_the_wait_for_the_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let mut request = HttpRequest::new(Method::Get, format!("http://{address}/silent"));
        request.timeout = Some(Duration::from_millis(200));
        let error = ReqwestTransport::new()
            .unwrap()
            .send_streaming(request)
            .await
            .unwrap_err();
        assert!(error.is_timeout(), "{error}");
        server.abort();
    }
}

/// Streams 200 MB generated as it is read, and prints the peak resident
/// memory of the process, then reads the same file whole for comparison.
/// `cargo test --release -p shopify_api --all-features --test bulk_stream -- --ignored --nocapture`
#[tokio::test]
#[ignore = "a measure, not a check: slow, and the numbers depend on the machine"]
async fn measure_the_memory_of_a_200_mb_export() {
    fn resident_kib() -> u64 {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
    let line = format!(
        "{{\"id\":\"gid://shopify/Product/1234567890\",\"title\":\"{}\",\"price\":\"19.99\"}}\n",
        "x".repeat(150)
    );
    let chunk = line.repeat(256).into_bytes();
    let chunks = 200_000_000 / chunk.len();
    let total = chunk.len() * chunks;

    #[derive(Deserialize)]
    #[allow(dead_code, reason = "decoded for the memory it takes, never read")]
    struct Row {
        id: String,
        title: String,
        price: String,
    }
    let mock = MockTransport::new();
    mock.push_chunks(200, std::iter::repeat_n(chunk.clone(), chunks));
    let shopify = client(&mock);

    let before = resident_kib();
    let mut rows = shopify
        .download_bulk_results_stream::<Row>(RESULTS_URL)
        .await
        .unwrap();
    let mut count = 0usize;
    let mut peak = before;
    while let Some(row) = rows.next_row().await {
        row.unwrap();
        count += 1;
        if count.is_multiple_of(20_000) {
            peak = peak.max(resident_kib());
        }
    }
    assert_eq!(count, 256 * chunks);
    println!(
        "streamed {} MB, {count} rows: resident {} KiB before, {} KiB at the peak (+{} KiB)",
        total / 1_000_000,
        before,
        peak,
        peak - before
    );

    let body = chunk.repeat(chunks);
    let rows: Vec<Row> = parse_jsonl(&body).unwrap();
    println!(
        "read whole: resident {} KiB with the file and its {} rows in memory",
        resident_kib(),
        rows.len()
    );
}
