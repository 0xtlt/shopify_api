//! The results of a bulk operation, read line by line as they download.

use std::collections::HashSet;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Range;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::{FusedStream, Stream};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::transport::BodyStream;
use crate::{Error, Result};

/// The longest line [`BulkRows`] and [`BulkGroups`] read by default: 64 MiB.
pub const DEFAULT_MAX_LINE_LENGTH: usize = 64 * 1024 * 1024;

/// What the buffer may keep, whatever it holds: a buffer more than four
/// times what it holds, and than this, shrinks after a long line.
const KEPT_CAPACITY: usize = 64 * 1024;

/// A line: its number, from 1, and where it is in the buffer.
type Line = (usize, Range<usize>);

/// A top-level object and its children.
type Grouped<P, C> = Result<(P, Vec<C>)>;

/// The lines of a JSONL body, split as its chunks arrive. It holds the
/// line being read and the rest of the chunk it ends in, never the file.
struct Lines {
    body: Option<BodyStream>,
    /// The address, without its query string, for errors.
    url: String,
    buffer: Vec<u8>,
    /// Where the line being read starts in `buffer`.
    start: usize,
    /// How many bytes of that line were searched for its end already.
    scanned: usize,
    /// The number of the last line taken, from 1.
    line: usize,
    max: usize,
    done: bool,
}

impl Lines {
    fn new(body: Option<BodyStream>, url: &str) -> Self {
        Self {
            done: body.is_none(),
            body,
            url: crate::error::without_query(url),
            buffer: Vec::new(),
            start: 0,
            scanned: 0,
            line: 0,
            max: DEFAULT_MAX_LINE_LENGTH,
        }
    }

    /// Stops reading: what is left of the body is dropped.
    fn finish(&mut self) {
        self.done = true;
        self.body = None;
        self.buffer = Vec::new();
        self.start = 0;
        self.scanned = 0;
    }

    fn too_long(&mut self, line: usize) -> Error {
        self.finish();
        Error::Bulk(format!(
            "line {line} of the results is longer than {} bytes",
            self.max
        ))
    }

    /// The next line that is not blank: its number and where it is in
    /// `buffer`, until the next poll.
    fn poll_line(&mut self, context: &mut Context<'_>) -> Poll<Option<Result<Line>>> {
        while !self.done {
            let from = self.start + self.scanned;
            if let Some(offset) = self.buffer[from..].iter().position(|byte| *byte == b'\n') {
                let range = self.start..from + offset;
                self.start = range.end + 1;
                self.scanned = 0;
                self.line += 1;
                if range.len() > self.max {
                    return Poll::Ready(Some(Err(self.too_long(self.line))));
                }
                if !is_blank(&self.buffer[range.clone()]) {
                    return Poll::Ready(Some(Ok((self.line, range))));
                }
                continue;
            }
            self.scanned = self.buffer.len() - self.start;
            if self.scanned > self.max {
                return Poll::Ready(Some(Err(self.too_long(self.line + 1))));
            }
            let Some(body) = self.body.as_mut() else {
                let range = self.start..self.buffer.len();
                self.start = range.end;
                self.scanned = 0;
                self.done = true;
                if range.is_empty() {
                    break;
                }
                self.line += 1;
                if is_blank(&self.buffer[range.clone()]) {
                    break;
                }
                return Poll::Ready(Some(Ok((self.line, range))));
            };
            match body.as_mut().poll_next(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => self.body = None,
                Poll::Ready(Some(Err(source))) => {
                    self.finish();
                    return Poll::Ready(Some(Err(Error::transport(&self.url, source))));
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buffer.drain(..self.start);
                    self.start = 0;
                    let needed = (self.buffer.len() + chunk.len()).max(KEPT_CAPACITY);
                    if self.buffer.capacity() > 4 * needed {
                        self.buffer.shrink_to(needed);
                    }
                    self.buffer.extend_from_slice(&chunk);
                }
            }
        }
        self.buffer = Vec::new();
        Poll::Ready(None)
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.buffer.capacity()
    }
}

/// A line of whitespace only, skipped as [`parse_jsonl`](super::parse_jsonl) skips it.
fn is_blank(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

impl fmt::Debug for Lines {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lines")
            .field("url", &self.url)
            .field("line", &self.line)
            .field("max", &self.max)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// The results of a bulk operation, each line read as `T` as the file
/// downloads. [`Shopify::bulk_query_stream`](crate::Shopify::bulk_query_stream)
/// and [`Shopify::download_bulk_results_stream`](crate::Shopify::download_bulk_results_stream)
/// return it.
///
/// It holds one line at a time, and the rest of the chunk that line ends
/// in: the memory it takes follows the longest line, not the file. Read it
/// with [`BulkRows::next_row`], or as a [`Stream`].
///
/// Lines are split on `\n`; a `\r` before it, a last line without one and
/// blank lines are fine, and blank lines are skipped but counted.
///
/// # Errors
///
/// - A line that does not read as `T` is an [`Error::Decode`] that gives
///   its line number and nothing of its content (see [`Error`]). The
///   stream **goes on** with the next line: skip the row, or stop.
/// - A line longer than [`BulkRows::max_line_length`] is an [`Error::Bulk`],
///   and a download that fails halfway an [`Error::Transport`]. The stream
///   **ends** after either.
#[must_use = "a stream does nothing until it is read"]
pub struct BulkRows<T> {
    lines: Lines,
    row: PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for BulkRows<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BulkRows")
            .field("lines", &self.lines)
            .finish()
    }
}

impl<T> BulkRows<T> {
    pub(crate) fn new(body: Option<BodyStream>, url: &str) -> Self {
        Self {
            lines: Lines::new(body, url),
            row: PhantomData,
        }
    }

    /// Refuses a line longer than `bytes`, newline left out, instead of
    /// [`DEFAULT_MAX_LINE_LENGTH`]: a file without newlines would otherwise
    /// be held whole.
    pub fn max_line_length(mut self, bytes: usize) -> Self {
        self.lines.max = bytes;
        self
    }

    /// The number of the last line read, from 1, blank lines included.
    pub fn line(&self) -> usize {
        self.lines.line
    }
}

impl<T: DeserializeOwned> BulkRows<T> {
    /// The next row, `None` at the end of the file.
    pub async fn next_row(&mut self) -> Option<Result<T>> {
        std::future::poll_fn(|context| Pin::new(&mut *self).poll_next(context)).await
    }

    /// Every row, up to the first error.
    pub(crate) async fn collect(mut self) -> Result<Vec<T>> {
        let mut rows = Vec::new();
        while let Some(row) = self.next_row().await {
            rows.push(row?);
        }
        Ok(rows)
    }
}

impl<T: DeserializeOwned> Stream for BulkRows<T> {
    type Item = Result<T>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Result<T>>> {
        let lines = &mut self.get_mut().lines;
        lines.poll_line(context).map(|line| {
            line.map(|line| {
                let (number, range) = line?;
                crate::decode::from_jsonl_line(&lines.buffer[range], number)
            })
        })
    }
}

impl<T: DeserializeOwned> FusedStream for BulkRows<T> {
    fn is_terminated(&self) -> bool {
        self.lines.done
    }
}

/// What [`BulkGroups`] reads of each line to place it.
#[derive(Deserialize)]
struct Keys {
    id: Option<String>,
    #[serde(rename = "__parentId")]
    parent_id: Option<String>,
}

struct Group<P, C> {
    parent: P,
    children: Vec<C>,
    /// The IDs of the objects of the group, the children of the next lines
    /// belong to.
    ids: HashSet<String>,
}

/// The results of a grouped bulk query, each top-level object with the
/// objects of its nested connections, as the file downloads.
/// [`Shopify::bulk_query_grouped`](crate::Shopify::bulk_query_grouped) and
/// [`Shopify::download_bulk_results_grouped`](crate::Shopify::download_bulk_results_grouped)
/// return it.
///
/// A line without `__parentId` is a top-level object, read as `P`; a line
/// with one is a child, read as `C`. Each item is a top-level object and
/// its children, in the order of the file: the item is complete when the
/// next top-level object, or the end of the file, arrives. One group is
/// held at a time.
///
/// # Nested connections two levels deep
///
/// Shopify allows connections two levels deep: the objects of the second
/// level carry the `__parentId` of an object of the first, not of the
/// top-level one. They are in the same list as the first level, in the
/// order of the file: read `__parentId` in `C` to put them back in place.
///
/// # The order it needs
///
/// It needs each child after its parent, before the next top-level object:
/// the order Shopify writes with `groupObjects`, which
/// [`Shopify::bulk_query_grouped`](crate::Shopify::bulk_query_grouped) asks
/// for. Without it, Shopify only promises a child after its parent,
/// anywhere further in the file. A child whose parent is neither the
/// current top-level object nor one of its children is an [`Error::Bulk`]
/// rather than a child put in the wrong group.
///
/// # Errors
///
/// Any error ends the stream, a line that does not read as `P` or `C`
/// included: the lines after it could not be placed for sure. The errors
/// are those of [`BulkRows`], and the child out of place above.
#[must_use = "a stream does nothing until it is read"]
pub struct BulkGroups<P, C> {
    lines: Lines,
    group: Option<Group<P, C>>,
}

// The group is never pinned: it is only reached through `&mut`.
impl<P, C> Unpin for BulkGroups<P, C> {}

impl<P, C> fmt::Debug for BulkGroups<P, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BulkGroups")
            .field("lines", &self.lines)
            .field(
                "children",
                &self.group.as_ref().map(|group| group.children.len()),
            )
            .finish()
    }
}

impl<P, C> BulkGroups<P, C> {
    pub(crate) fn new(body: Option<BodyStream>, url: &str) -> Self {
        Self {
            lines: Lines::new(body, url),
            group: None,
        }
    }

    /// [`BulkRows::max_line_length`].
    pub fn max_line_length(mut self, bytes: usize) -> Self {
        self.lines.max = bytes;
        self
    }

    /// The number of the last line read, from 1, blank lines included.
    pub fn line(&self) -> usize {
        self.lines.line
    }

    fn fail(&mut self, error: Error) -> Poll<Option<Grouped<P, C>>> {
        self.lines.finish();
        self.group = None;
        Poll::Ready(Some(Err(error)))
    }
}

impl<P: DeserializeOwned, C: DeserializeOwned> BulkGroups<P, C> {
    /// The next top-level object and its children, `None` at the end of the file.
    pub async fn next_group(&mut self) -> Option<Grouped<P, C>> {
        std::future::poll_fn(|context| Pin::new(&mut *self).poll_next(context)).await
    }
}

impl<P: DeserializeOwned, C: DeserializeOwned> Stream for BulkGroups<P, C> {
    type Item = Grouped<P, C>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            let (number, range) = match this.lines.poll_line(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    let group = this.group.take();
                    return Poll::Ready(group.map(|group| Ok((group.parent, group.children))));
                }
                Poll::Ready(Some(Err(error))) => return this.fail(error),
                Poll::Ready(Some(Ok(line))) => line,
            };
            let line = &this.lines.buffer[range];
            let keys = match crate::decode::from_jsonl_line::<Keys>(line, number) {
                Ok(keys) => keys,
                Err(error) => return this.fail(error),
            };
            match keys.parent_id {
                None => {
                    let parent = match crate::decode::from_jsonl_line::<P>(line, number) {
                        Ok(parent) => parent,
                        Err(error) => return this.fail(error),
                    };
                    let next = Group {
                        parent,
                        children: Vec::new(),
                        ids: keys.id.into_iter().collect(),
                    };
                    if let Some(done) = this.group.replace(next) {
                        return Poll::Ready(Some(Ok((done.parent, done.children))));
                    }
                }
                Some(parent_id) => {
                    if !this
                        .group
                        .as_ref()
                        .is_some_and(|group| group.ids.contains(&parent_id))
                    {
                        let error = Error::Bulk(format!(
                            "line {number} of the results is a child of neither the top-level object before it nor one of its children: run the query with `groupObjects`"
                        ));
                        return this.fail(error);
                    }
                    let child = match crate::decode::from_jsonl_line::<C>(line, number) {
                        Ok(child) => child,
                        Err(error) => return this.fail(error),
                    };
                    if let Some(group) = this.group.as_mut() {
                        group.ids.extend(keys.id);
                        group.children.push(child);
                    }
                }
            }
        }
    }
}

impl<P: DeserializeOwned, C: DeserializeOwned> FusedStream for BulkGroups<P, C> {
    fn is_terminated(&self) -> bool {
        self.lines.done && self.group.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{TransportError, chunks};

    fn rows_of<T>(parts: Vec<Vec<u8>>) -> BulkRows<T> {
        BulkRows::new(
            Some(chunks(parts.into_iter().map(Ok))),
            "https://storage.test/r.jsonl",
        )
    }

    async fn all<T: DeserializeOwned>(mut rows: BulkRows<T>) -> Vec<Result<T>> {
        let mut out = Vec::new();
        while let Some(row) = rows.next_row().await {
            out.push(row);
        }
        out
    }

    /// The rows and errors as text: what a split must not change.
    fn shown(results: &[Result<serde_json::Value>]) -> Vec<String> {
        results
            .iter()
            .map(|result| match result {
                Ok(value) => value.to_string(),
                Err(error) => format!("error: {error}"),
            })
            .collect()
    }

    /// What `parse_jsonl` gives line by line: each line on its own, numbered.
    fn buffered(body: &[u8]) -> Vec<String> {
        body.split(|byte| *byte == b'\n')
            .enumerate()
            .filter(|(_, line)| !is_blank(line))
            .map(|(index, line)| {
                match crate::decode::from_jsonl_line::<serde_json::Value>(line, index + 1) {
                    Ok(value) => value.to_string(),
                    Err(error) => format!("error: {error}"),
                }
            })
            .collect()
    }

    const BODY: &[u8] = "{\"id\":\"gid://shopify/Product/1\",\"title\":\"Café ☕ 𝄞\"}\r\n\
        \r\n\
        {\"id\":\"gid://shopify/ProductVariant/2\",\"__parentId\":\"gid://shopify/Product/1\"}\n\
        \n   \n\
        {\"broken\": tru}\n\
        {\"id\":3,\"title\":\"Ünïcødé\"}"
        .as_bytes();

    #[tokio::test]
    async fn every_split_in_two_or_three_reads_as_the_whole_file() {
        let expected = buffered(BODY);
        assert_eq!(expected.len(), 4);
        assert!(expected[2].starts_with("error: "), "{expected:?}");
        for first in 0..=BODY.len() {
            let parts = vec![BODY[..first].to_vec(), BODY[first..].to_vec()];
            assert_eq!(
                shown(&all(rows_of(parts)).await),
                expected,
                "split at {first}"
            );
            for second in first..=BODY.len() {
                let parts = vec![
                    BODY[..first].to_vec(),
                    BODY[first..second].to_vec(),
                    BODY[second..].to_vec(),
                ];
                assert_eq!(
                    shown(&all(rows_of(parts)).await),
                    expected,
                    "split at {first} and {second}"
                );
            }
        }
    }

    #[tokio::test]
    async fn random_splits_read_as_the_whole_file() {
        let mut body = Vec::new();
        for index in 0..300 {
            let line = serde_json::json!({ "id": index, "title": "é".repeat(index % 7), "n": [index, index] });
            body.extend_from_slice(line.to_string().as_bytes());
            body.extend_from_slice(if index.is_multiple_of(3) {
                b"\r\n"
            } else {
                b"\n"
            });
            if index.is_multiple_of(11) {
                body.extend_from_slice(b"\n");
            }
        }
        let expected = buffered(&body);
        assert_eq!(expected.len(), 300);
        let whole: Vec<serde_json::Value> = crate::bulk::parse_jsonl(&body).unwrap();
        let whole: Vec<String> = whole.iter().map(ToString::to_string).collect();
        assert_eq!(whole, expected);
        // A fixed xorshift: the same splits on every run.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..200 {
            let mut parts = Vec::new();
            let mut at = 0;
            while at < body.len() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let size = (state % 97) as usize;
                let end = (at + size).min(body.len());
                parts.push(body[at..end].to_vec());
                at = end;
            }
            assert_eq!(shown(&all(rows_of(parts)).await), expected);
        }
        let one_byte_each = body.iter().map(|byte| vec![*byte]).collect();
        assert_eq!(shown(&all(rows_of(one_byte_each)).await), expected);
    }

    #[tokio::test]
    async fn a_last_blank_line_without_a_newline_is_counted() {
        for (body, lines) in [
            (&b"{}\n \r"[..], 2),
            (b"{}\n\n\t", 3),
            (b"{}\n", 1),
            (b"{}", 1),
        ] {
            let mut rows = rows_of::<serde_json::Value>(vec![body.to_vec()]);
            assert!(rows.next_row().await.unwrap().is_ok());
            assert!(rows.next_row().await.is_none());
            assert_eq!(rows.line(), lines, "{body:?}");
        }
    }

    #[tokio::test]
    async fn a_line_longer_than_the_limit_ends_the_stream() {
        // Too long whether its newline comes with it or not.
        for parts in [
            vec![b"{\"a\":1}\n{\"b\":\"0123456789\"}\n{\"c\":3}\n".to_vec()],
            vec![
                b"{\"a\":1}\n{\"b\":\"01234".to_vec(),
                b"56789\"}\n{\"c\":3}\n".to_vec(),
            ],
            vec![b"{\"a\":1}\n{\"b\":\"0123456789\"}".to_vec()],
        ] {
            let mut rows = rows_of::<serde_json::Value>(parts).max_line_length(12);
            assert_eq!(rows.next_row().await.unwrap().unwrap()["a"], 1);
            let error = rows.next_row().await.unwrap().unwrap_err();
            assert_eq!(
                error.to_string(),
                "bulk operation: line 2 of the results is longer than 12 bytes"
            );
            assert!(rows.next_row().await.is_none());
            assert!(rows.is_terminated());
        }
        // A line of exactly the limit is read; `\r` counts, `\n` does not.
        let mut rows = rows_of::<serde_json::Value>(vec![b"{\"b\":\"12345\"}\r\n".to_vec()])
            .max_line_length(14);
        assert_eq!(rows.next_row().await.unwrap().unwrap()["b"], "12345");
    }

    #[tokio::test]
    async fn a_file_without_newlines_is_not_held_whole() {
        let chunk = vec![b'x'; 64 * 1024];
        // 1 GiB, never produced: the limit stops it after 1 MiB.
        let endless = std::iter::repeat_n(chunk, 16 * 1024).map(Ok);
        let mut rows: BulkRows<serde_json::Value> =
            BulkRows::new(Some(chunks(endless)), "https://storage.test/r.jsonl")
                .max_line_length(1024 * 1024);
        let error = rows.next_row().await.unwrap().unwrap_err();
        assert!(matches!(error, Error::Bulk(_)), "{error:?}");
        assert!(rows.next_row().await.is_none());
    }

    #[tokio::test]
    async fn a_body_that_fails_halfway_ends_the_stream() {
        let parts = vec![
            Ok(b"{\"a\":1}\n{\"a\":".to_vec()),
            Err(TransportError::new("connection reset")),
            Ok(b"2}\n".to_vec()),
        ];
        let mut rows: BulkRows<serde_json::Value> = BulkRows::new(
            Some(chunks(parts)),
            "https://storage.test/r.jsonl?sig=SECRET",
        );
        assert_eq!(rows.next_row().await.unwrap().unwrap()["a"], 1);
        let error = rows.next_row().await.unwrap().unwrap_err();
        assert_eq!(
            error.to_string(),
            "request to https://storage.test/r.jsonl failed: connection reset"
        );
        assert!(rows.next_row().await.is_none());
    }

    #[tokio::test]
    async fn a_decode_error_names_its_line_shows_nothing_of_it_and_the_stream_goes_on() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Row {
            id: u64,
        }
        let body = b"{\"id\":1}\n\n{\"id\":\"shpat_SECRET\"}\n{\"id\":3}\n".to_vec();
        let results = all(rows_of::<Row>(vec![body])).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap().id, 1);
        let error = results[1].as_ref().unwrap_err();
        let shown = format!("{error}\n{error:?}\n{error:#?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert_eq!(
            error.to_string(),
            "Shopify's answer does not match the operation at `id`: invalid type: string, expected u64 at line 3 column 20"
        );
        assert!(error.sensitive_detail().unwrap().contains("shpat_SECRET"));
        assert_eq!(results[2].as_ref().unwrap().id, 3);
    }

    /// The buffer follows the longest line, whatever the size of the file.
    #[tokio::test]
    async fn memory_follows_the_longest_line_not_the_file() {
        let line = format!(
            "{{\"id\":\"gid://shopify/Product/1\",\"title\":\"{}\"}}\n",
            "a".repeat(200)
        );
        let chunk = line.repeat(64).into_bytes();
        let chunk_len = chunk.len();
        // About 20 MB, generated as it is read.
        let count = 20_000_000 / chunk_len;
        let body = std::iter::repeat_n(chunk, count).map(Ok);
        let mut rows: BulkRows<serde::de::IgnoredAny> =
            BulkRows::new(Some(chunks(body)), "https://storage.test/r.jsonl");
        let mut read = 0;
        let mut largest = 0;
        while let Some(row) = rows.next_row().await {
            row.unwrap();
            read += 1;
            largest = largest.max(rows.lines.capacity());
        }
        assert_eq!(read, count * 64);
        // A chunk and a partial line, with the growth of a `Vec`.
        assert!(
            largest <= 2 * (chunk_len + line.len()),
            "{largest} bytes held"
        );
    }

    #[tokio::test]
    async fn the_buffer_of_a_huge_line_is_released_once_it_is_read() {
        let huge = format!("{{\"a\":\"{}\"}}\n", "x".repeat(16 * 1024 * 1024));
        let tiny = b"{\"b\":1}\n".to_vec();
        let parts = vec![huge.into_bytes(), tiny.clone(), tiny.clone(), tiny.clone()];
        let mut rows = rows_of::<serde::de::IgnoredAny>(parts);
        rows.next_row().await.unwrap().unwrap();
        assert!(rows.lines.capacity() > 16 * 1024 * 1024);
        rows.next_row().await.unwrap().unwrap();
        assert!(
            rows.lines.capacity() <= 1024 * 1024,
            "{} bytes held after a tiny line",
            rows.lines.capacity()
        );
        while let Some(row) = rows.next_row().await {
            row.unwrap();
        }
        assert_eq!(rows.lines.capacity(), 0);
    }

    /// A buffer a few times larger than its chunks is kept: no
    /// reallocation on each chunk.
    #[tokio::test]
    async fn the_buffer_is_kept_while_lines_stay_alike() {
        let line = format!("{{\"a\":\"{}\"}}\n", "x".repeat(300 * 1024));
        let parts: Vec<Vec<u8>> = std::iter::repeat_n(line.into_bytes(), 8).collect();
        let mut rows = rows_of::<serde::de::IgnoredAny>(parts);
        rows.next_row().await.unwrap().unwrap();
        let first = rows.lines.buffer.as_ptr();
        for _ in 0..7 {
            rows.next_row().await.unwrap().unwrap();
            assert_eq!(rows.lines.buffer.as_ptr(), first);
        }
    }

    fn groups_of<P, C>(body: &str) -> BulkGroups<P, C> {
        BulkGroups::new(
            Some(chunks(
                body.as_bytes()
                    .chunks(5)
                    .map(|part| Ok(part.to_vec()))
                    .collect::<Vec<_>>(),
            )),
            "https://storage.test/r.jsonl",
        )
    }

    async fn all_groups(
        mut groups: BulkGroups<serde_json::Value, serde_json::Value>,
    ) -> Vec<Result<(serde_json::Value, Vec<serde_json::Value>)>> {
        let mut out = Vec::new();
        while let Some(group) = groups.next_group().await {
            out.push(group);
        }
        out
    }

    fn ids(values: &[serde_json::Value]) -> Vec<&str> {
        values
            .iter()
            .map(|value| value["id"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn groups_hold_each_top_level_object_with_its_children() {
        let body = r#"{"id":"P1"}
{"id":"V1","__parentId":"P1"}
{"id":"V2","__parentId":"P1"}

{"id":"P2"}
{"id":"P3"}
{"id":"V3","__parentId":"P3"}"#;
        let groups = all_groups(groups_of(body)).await;
        let groups: Vec<_> = groups.into_iter().map(Result::unwrap).collect();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0["id"], "P1");
        assert_eq!(ids(&groups[0].1), ["V1", "V2"]);
        assert_eq!(groups[1].0["id"], "P2");
        assert!(groups[1].1.is_empty());
        assert_eq!(ids(&groups[2].1), ["V3"]);
    }

    #[tokio::test]
    async fn the_second_level_joins_the_group_of_its_top_level_object() {
        let body = r#"{"id":"P1"}
{"id":"V1","__parentId":"P1"}
{"id":"L1","__parentId":"V1"}
{"id":"L2","__parentId":"V1"}
{"id":"V2","__parentId":"P1"}
{"id":"L3","__parentId":"V2"}
{"id":"M1","__parentId":"P1"}
{"id":"P2"}
{"id":"V3","__parentId":"P2"}
{"id":"L4","__parentId":"V3"}
"#;
        let groups: Vec<_> = all_groups(groups_of(body))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(groups.len(), 2);
        assert_eq!(ids(&groups[0].1), ["V1", "L1", "L2", "V2", "L3", "M1"]);
        assert_eq!(groups[0].1[2]["__parentId"], "V1");
        assert_eq!(ids(&groups[1].1), ["V3", "L4"]);
    }

    #[tokio::test]
    async fn a_child_out_of_its_group_is_an_error_that_ends_the_stream() {
        // V3's parent came before P2: the order without `groupObjects`.
        let body = r#"{"id":"P1"}
{"id":"V1","__parentId":"P1"}
{"id":"P2"}
{"id":"V3","__parentId":"P1"}
{"id":"P3"}
"#;
        let groups = all_groups(groups_of(body)).await;
        assert_eq!(groups.len(), 2);
        let (parent, children) = groups[0].as_ref().unwrap();
        assert_eq!(parent["id"], "P1");
        assert_eq!(ids(children), ["V1"]);
        let error = groups[1].as_ref().unwrap_err();
        assert_eq!(
            error.to_string(),
            "bulk operation: line 4 of the results is a child of neither the top-level object before it nor one of its children: run the query with `groupObjects`"
        );

        // Before any top-level object, or to an unknown parent.
        for body in [
            "{\"id\":\"V1\",\"__parentId\":\"P1\"}\n{\"id\":\"P1\"}\n",
            "{\"id\":\"P1\"}\n{\"id\":\"V1\",\"__parentId\":\"P9\"}\n",
        ] {
            let groups = all_groups(groups_of(body)).await;
            assert_eq!(groups.len(), 1, "{body}");
            assert!(matches!(groups[0], Err(Error::Bulk(_))), "{body}");
        }
    }

    #[tokio::test]
    async fn groups_read_typed_parents_and_children_and_hide_bad_values() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Product {
            id: String,
        }
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Variant {
            id: String,
            price: u64,
        }
        let body = "{\"id\":\"P1\"}\n{\"id\":\"V1\",\"__parentId\":\"P1\",\"price\":\"shpat_SECRET\"}\n{\"id\":\"P2\"}\n";
        let mut groups = groups_of::<Product, Variant>(body);
        let error = groups.next_group().await.unwrap().unwrap_err();
        let shown = format!("{error}\n{error:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("line 2"), "{shown}");
        assert!(groups.next_group().await.is_none());
        assert!(groups.is_terminated());
    }
}
