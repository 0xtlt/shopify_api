use shopify_api::{Maybe, input};

fn main() {
    let item = input!(InventoryTransferLineItemInput { inventoryItemId: "gid://shopify/InventoryItem/1", quantity: 2 });
    let line_items: Maybe<Vec<_>> = Maybe::Value(vec![item]);
    let _ = input!(InventoryTransferCreateInput { lineItems?: line_items });
}
