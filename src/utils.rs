use std::future::Future;

/// Retry function for async functions
/// # Example
/// ```
/// use shopify_api::utils::retry_async;
/// use std::io::{Error, ErrorKind};
///
/// async fn my_async_function(args: &(String, u8)) -> Result<(), Error> {
///    Err(Error::new(ErrorKind::Other, "Error"))
/// }
///
/// #[tokio::main]
/// async fn main() {
///   let result = retry_async(3, my_async_function, &(String::from("test"), 1)).await;
///  assert!(result.is_err());
/// }
/// ```
/// # Errors
/// This function returns an error if the async function returns an error
/// # Panics
/// This function panics if the number of retries is 0
pub async fn retry_async<'a, Fut, F, Args, Out, ErrOut>(
    max_retries: u64,
    func: Fut,
    args: &'a Args,
) -> Result<Out, ErrOut>
where
    Fut: Fn(&'a Args) -> F,
    F: Future<Output = Result<Out, ErrOut>>,
    ErrOut: std::fmt::Debug,
{
    if max_retries == 0 {
        panic!("Max retries cannot be 0");
    }

    let mut count: u64 = 0;
    let mut result: Result<Out, ErrOut> = func(args).await;

    while count < max_retries - 1 {
        let executed_func = func(args);

        result = executed_func.await;
        if result.is_ok() {
            return result;
        }

        count += 1;
    }

    result
}

/// Retry function for sync functions
/// # Example
/// ```
/// use shopify_api::utils::retry_sync;
/// use std::io::{Error, ErrorKind};
///   
/// fn my_sync_function(args: &(String, u8)) -> Result<(), Error> {
///   Err(Error::new(ErrorKind::Other, "Error"))
/// }
///
/// fn main() {
///  let result = retry_sync(3, my_sync_function, &(String::from("test"), 1));
///  assert!(result.is_err());
/// }
/// ```
/// # Errors
/// This function returns an error if the sync function returns an error
/// # Panics
/// This function panics if the number of retries is 0
pub fn retry_sync<'a, F, Args, Out, ErrOut>(
    max_retries: u64,
    func: F,
    args: &'a Args,
) -> Result<Out, ErrOut>
where
    F: Fn(&'a Args) -> Result<Out, ErrOut>,
    ErrOut: std::fmt::Debug,
{
    if max_retries == 0 {
        panic!("Max retries cannot be 0");
    }

    let mut count: u64 = 0;
    let mut result: Result<Out, ErrOut> = func(args);

    while count < max_retries - 1 {
        let executed_func = func(args);

        result = executed_func;
        if result.is_ok() {
            return result;
        }

        count += 1;
    }

    result
}
