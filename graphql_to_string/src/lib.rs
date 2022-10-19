extern crate proc_macro;

use std::str::FromStr;

use proc_macro::TokenStream;
use quote::quote;
use syn::{self, parse_macro_input, Data, DataEnum, DeriveInput, Field, Path, Type, TypePath};

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

fn extend_token_stream(obj: &mut TokenStream, ts: TokenStream) {
    let mut ts = ts;
    obj.extend(ts);
}

// Input is a enum that can contains a vector on enum
// Output is a string that can be used in a graphql query
#[proc_macro_derive(ToGraphStringDerive)]
pub fn to_graph_string_derive(tokens: TokenStream) -> TokenStream {
    let tok_clone = tokens.clone();
    let input = parse_macro_input!(tokens as DeriveInput);

    let name = input.ident;

    let variants_punct = match input.data {
        Data::Enum(DataEnum { variants, .. }) => variants,
        _ => panic!("Only enums can be annotated with ToGraphString"),
    };

    // let mut code = TokenStream::from(tok_clone);
    let mut code = TokenStream::new();

    let mut tmp_code = format!(r#"impl ToGraphString for {name} {{
        fn to_graph_string(&self) -> String {{
            match self {{
            "#, name = name);


    for variant in variants_punct.iter() {
        println!("variant: {:#?}", variant);
        let variant_ident = &variant.ident;
        let variant_fields = &variant.fields;
        let variant_field = variant_fields.iter().next();

        if variant_field.is_some() && is_vec(variant_field.unwrap()) {
            // TODO: do it
        } else {
            tmp_code.push_str(&format!(r#"
                {name}::{variant_ident} => stringify!({variant_ident}).to_string(),
            "#, name = name, variant_ident = variant_ident));
        }
        //     let tmp: TokenStream = quote! {
        //         impl ToGraphString for #name {
        //             fn to_graph_string(&self) -> String {
        //                 let mut result = String::new();
        //                 match self {
        //                     #name::#variant_ident(v) => {
        //                         for i in v.iter() {
        //                             result.push_str(&i.to_graph_string());
        //                         }
        //                     }
        //                 };
        //                 result
        //             }
        //         }
        //     }
        //     .into();
        //     code.extend(tmp);
        // } else {
        //     let tmp: TokenStream = quote! {
        //         impl ToGraphString for #name {
        //             fn to_graph_string(&self) -> String {
        //                 let mut result = String::new();
        //                 match self {
        //                     #name::#variant_ident(v) => {
        //                         result.push_str(&v.to_graph_string());
        //                     }
        //                 };
        //                 result
        //             }
        //         }
        //     }
        //     .into();

        //     code.extend(tmp);
        // }

        println!("variant_ident: {:#?}", variant_ident);
    }

    tmp_code.push_str(r#" }
}
}
            "#);

    code.extend(TokenStream::from_str(&tmp_code).unwrap());

    // End code
    let c = code.clone().to_string();
    println!("code: {:?}", c);

    code
}
