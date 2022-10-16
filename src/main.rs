use chrono::TimeZone;
use shopify_api::{get_end_of_support_date, ShopifyAPIVersion};

fn main() {
    assert_eq!(
        get_end_of_support_date(&ShopifyAPIVersion::V2023_01),
        chrono::Utc.ymd(2023, 1, 31).and_hms(23, 59, 59)
    );
}
