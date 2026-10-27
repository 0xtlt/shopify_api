shopify_api::graphql! {
    mod nested = bulk(bulk("{ products { edges { node { id } } } }"));
}

fn main() {}
