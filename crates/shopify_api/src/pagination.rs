//! Cursor pagination over a connection.

use serde::de::DeserializeOwned;

use crate::{Error, Operation, Result, Shopify};

/// One page of a connection: its items and its `pageInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Page<T> {
    pub items: Vec<T>,
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, has_next_page: bool, end_cursor: Option<String>) -> Self {
        Self {
            items,
            has_next_page,
            end_cursor,
        }
    }

    /// The items converted, the page info kept.
    pub fn map<U>(self, convert: impl FnMut(T) -> U) -> Page<U> {
        Page {
            items: self.items.into_iter().map(convert).collect(),
            has_next_page: self.has_next_page,
            end_cursor: self.end_cursor,
        }
    }
}

impl Shopify {
    /// Runs `operation` page after page and collects every item.
    ///
    /// `operation` receives the cursor to start after, `None` for the first
    /// page; `page` reads the items and the `pageInfo` out of each answer.
    ///
    /// ```no_run
    /// use shopify_api::{Page, graphql};
    ///
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let products = shopify
    ///     .paginate(
    ///         |after| {
    ///             graphql!(
    ///                 "query($after: String) {
    ///                    products(first: 250, after: $after) {
    ///                      nodes { id title }
    ///                      pageInfo { hasNextPage endCursor }
    ///                    }
    ///                  }",
    ///                 after = after,
    ///             )
    ///         },
    ///         |data| {
    ///             let page_info = data.products.page_info;
    ///             Page::new(data.products.nodes, page_info.has_next_page, page_info.end_cursor)
    ///         },
    ///     )
    ///     .await?;
    /// println!("{} products", products.len());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// For a large catalog, a [bulk operation](crate::bulk) reads everything
    /// at once, without the rate limit.
    pub async fn paginate<D, T, O, P>(&self, mut operation: O, mut page: P) -> Result<Vec<T>>
    where
        D: DeserializeOwned,
        O: FnMut(Option<String>) -> Operation<D>,
        P: FnMut(D) -> Page<T>,
    {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let data = self.execute(operation(cursor.clone())).await?;
            let current = page(data);
            items.extend(current.items);
            if !current.has_next_page {
                return Ok(items);
            }
            match current.end_cursor {
                Some(next) if cursor.as_ref() != Some(&next) => cursor = Some(next),
                _ => {
                    return Err(Error::decode("pageInfo.endCursor".to_string(), "`hasNextPage` is true but the cursor does not move: select `pageInfo { hasNextPage endCursor }` and pass the cursor as `after`".to_string(), None));
                }
            }
        }
    }
}
