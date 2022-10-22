extern crate proc_macro;

use std::str::FromStr;

use proc_macro::TokenStream;
use syn::{
    self, parse_macro_input, Data, DataEnum, DeriveInput, Field, Fields, FieldsNamed, Path, Type,
    TypePath,
};

fn is_vec(field: &Field) -> bool {
    match &field.ty {
        Type::Path(TypePath {
            path: Path { segments, .. },
            ..
        }) => {
            // segments is of Type syn::punctuated::Punctuated<PathSegment, _>
            if let Some(path_seg) = segments.first() {
                let ident = &path_seg.ident;
                return ident == "Vec";
            }
            false
        }
        _ => false,
    }
}

/// Function for each upper case letter -> _letter
/// # Example
/// ```
/// let s = "HelloWorld";
/// let s = to_snake_case(s);
/// assert_eq!(s, "hello_world");
/// ```
fn to_snake_case(s: &str) -> String {
    let mut res = String::new();
    for (index, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if index != 0 {
                res.push('_');
            }

            res.push(c.to_ascii_lowercase());
        } else {
            res.push(c);
        }
    }
    res
}

/// Function to pass the first letter to down case
/// # Example
/// ```
/// let s = "HelloWorld";
/// let s = to_first_lower(s);
/// assert_eq!(s, "helloWorld");
/// ```
fn to_first_lower(s: &str) -> String {
    let mut res = String::new();
    for (index, c) in s.chars().enumerate() {
        if index == 0 {
            res.push(c.to_ascii_lowercase());
        } else {
            res.push(c);
        }
    }
    res
}

// Input is a enum that can contains a vector on enum
// Output is a string that can be used in a graphql query
#[proc_macro_derive(ToGraphStringDerive)]
pub fn to_graph_string_derive(tokens: TokenStream) -> TokenStream {
    let input = parse_macro_input!(tokens as DeriveInput);

    let name = input.ident;

    let variants_punct = match input.data {
        Data::Enum(DataEnum { variants, .. }) => variants,
        _ => panic!("Only enums can be annotated with ToGraphString"),
    };

    // let mut code = TokenStream::from(tok_clone);
    let mut code = TokenStream::new();

    let mut tmp_code = format!(
        r#"impl ToGraphString for {name} {{
        fn to_graph_string(&self) -> String {{
            match self {{
            "#,
        name = name
    );

    for variant in variants_punct.iter() {
        let variant_ident = &variant.ident;
        let variant_fields = &variant.fields;
        let variant_field = variant_fields.iter().next();

        if variant_field.is_some() && is_vec(variant_field.unwrap()) {
            let variant_ident_name = to_first_lower(&variant_ident.to_string());
            tmp_code.push_str(&format!(
                r#"
                {name}::{variant_ident}(vector_var) => {{
                    let mut s = String::new();
                    s.push_str("{variant_ident_name}");
                    s.push_str(" {{");
                    for field in vector_var.iter().enumerate() {{
                        s.push('\n');
                        s.push_str(&field.1.to_graph_string());
                    }}
                    s.push_str("\n }}");
                    s
                }}
            "#,
                name = name,
                variant_ident = variant_ident,
                variant_ident_name = variant_ident_name
            ));
        } else if let Fields::Named(FieldsNamed { named, .. }) = variant_fields {
            let variant_ident_name = to_first_lower(&variant_ident.to_string());
            let mut param: Vec<&str> = Vec::new();

            for part in named.iter() {
                let key = part.ident.as_ref().unwrap();
                let ident_name = key.to_string();
                // "connector" will be a reserved keyword here to indicate a connection


                if ident_name == "connector" {
                } else {
                }
            }

            tmp_code.push_str(&format!(r#"
                {name}::{ident_name} {end}, 
                "#,
                name = name,
                ident_name = variant_ident_name,
                end = {
                    // end must returns something like that: "{helloworld} => {...}"
                    println!("p{:?}", part);


                    "{helloworld} => {}"
                }
            ));

            // todo!("Transform the named fields to a string and check for connector property");
        } else {
            let variant_ident_name = to_first_lower(&variant_ident.to_string());
            tmp_code.push_str(&format!(
                r#"
                {name}::{variant_ident} => stringify!({variant_ident_name}).to_string(),
            "#,
                name = name,
                variant_ident = variant_ident,
                variant_ident_name = variant_ident_name
            ));
        }
    }

    tmp_code.push_str(r#"}}}"#);

    println!("Final code: {}", tmp_code);

    code.extend(TokenStream::from_str(&tmp_code).unwrap());

    code
}
