extern crate proc_macro;

use std::collections::HashMap;
use std::str::FromStr;

use proc_macro::TokenStream;
use syn::{
    self, parse_macro_input, Data, DataEnum, DeriveInput, Field, Fields, FieldsNamed, Path,
    PathSegment, Token, Type, TypePath,
};

use syn::PathArguments::AngleBracketed;
use syn::Type::Path as PathType;

// TODO: document it
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

// TODO: document it
fn get_type_from_type(value: &syn::GenericArgument) -> Option<String> {
    match value {
        syn::GenericArgument::Type(value) => match value {
            syn::Type::Path(value_2) => {
                let segments = &value_2.path.segments;

                if segments.len() == 0 {
                    panic!("Not supported");
                }

                let type_str = &segments[0].ident.to_string();

                Some(type_str.clone())
            }
            _ => panic!("Not supported"),
        },
        _ => None,
    }
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

// TODO: document it
/// Function to returns the type of a variable
fn get_type(segments: &syn::punctuated::Punctuated<PathSegment, Token![::]>) -> (bool, String) {
    if segments.len() != 1 {
        panic!("Not allowed here");
    }

    let first_segment = &segments[0];
    let first_segment_name = &first_segment.ident.to_string();
    let first_segment_args = &first_segment.arguments;

    let is_option = first_segment_name == "Option";
    let mut type_str = String::new();

    if is_option {
        if let AngleBracketed(value) = first_segment_args {
            let args = &value.args;
            if args.len() == 0 {
                panic!("Not allowed here");
            }

            let first_arg = &args[0];
            let value = get_type_from_type(&first_arg).unwrap();
            type_str = value;
        } else {
            panic!("Not allowed here");
        }
    } else {
        // TODO: add an error message with a panic
        // only support primary types (int, uint, String, ...), no tupples/enum/struct
    }

    (is_option, type_str)
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
            let mut params: HashMap<String, (bool, String)> = HashMap::new();

            for part in named.iter() {
                let key = part.ident.as_ref().unwrap();
                let ident_name = key.to_string();

                // TODO: Support connector field to add sub fields query
                // "connector" will be a reserved keyword here to indicate a connection
                if ident_name == "connector_not_actually_supported" {
                } else {
                    let part_type = &part.ty;

                    if let PathType(content) = part_type {
                        let content_segments = &content.path.segments;

                        params.insert(ident_name, get_type(content_segments));
                    }
                }
            }

            tmp_code.push_str(&format!(
            r#"
            {name}::{ident_name} {end}, 
            "#,
            name = name,
            ident_name = variant_ident.to_string(),
            end = {
                let mut output_params_string: String = String::new();
                let mut output_inner_string: String =
                    String::from("let mut output_string: Vec<String> = vec![];");

                for (index, (name, options)) in params.iter().enumerate() {
                    // Params section
                    if index != 0 {
                        // If not 0, add a coma ','
                        output_params_string.push(',');
                    }

                    // Is an option = not required
                    if options.0 {
                        output_inner_string.push_str(&format!(
                            r#"
                            if let Some(value) = {name} {{
                                output_string.push(format!("{name}:{{:?}}", value));
                            }}
                        "#,
                            name = name
                        ));
                    } else {
                        // Required
                        output_inner_string.push_str(&format!(
                            r#"
                            output_string.push(format!("{name}:{{:?}}", {name}));
                        "#,
                            name = name
                        ));
                    }
                    output_params_string.push_str(name);
                }

                format!(
                    r#"{{{params}}} => {{
                    {content}

                    format!("{{}}({{}})", stringify!({variant_ident_name}).to_string(), output_string.join(", "))
                }}"#,
                    params = output_params_string,
                    content = output_inner_string
                )
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
    code.extend(TokenStream::from_str(&tmp_code).unwrap());

    code
}
