//! The `graphql!` and `input!` macros of `shopify_api`. Use them through
//! `shopify_api`, which re-exports and documents them.
#![forbid(unsafe_code)]

mod value;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use shopify_api_graphql::codegen::{DATA_TYPE, data_types, deprecation_warning, variables_types};
use shopify_api_graphql::literal::{self, Literals, Value, VariableValue};
use shopify_api_graphql::project::{metadata_snippet, project_schema};
use shopify_api_graphql::{
    MINIMUM_API_VERSION, Operation, OperationKind, SCHEMA_ENV, Schema, SchemaSource, Variable,
    analyze, is_supported_api_version, latest_api_version, schema_source,
};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Attribute, Ident, LitStr, Token, Visibility};

/// The schema of `shopify_api`'s own operations, relative to its `Cargo.toml`:
/// the Admin schema cut down to what they use. A crate that depends on
/// `shopify_api` selects a version for its operations, not for these.
const INTERNAL_SCHEMA_PATH: &str = "graphql/internal.schema.json";

/// Where the text of an operation comes from.
enum Source {
    /// `"query { … }"`.
    Literal(LitStr),
    /// `file("graphql/orders.graphql")`, relative to the crate's `Cargo.toml`.
    File(LitStr),
}

impl Parse for Source {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Ident) && input.peek2(syn::token::Paren) {
            let keyword: Ident = input.parse()?;
            if keyword != "file" {
                return Err(syn::Error::new(
                    keyword.span(),
                    "expected the GraphQL operation as a string literal, or `file(\"path.graphql\")`",
                ));
            }
            let content;
            syn::parenthesized!(content in input);
            let path: LitStr = content.parse()?;
            if !content.is_empty() {
                return Err(content.error("`file(…)` takes one path"));
            }
            return Ok(Self::File(path));
        }
        input.parse().map(Self::Literal).map_err(|error| {
            syn::Error::new(
                error.span(),
                "expected the GraphQL operation as a string literal, or `file(\"path.graphql\")`",
            )
        })
    }
}

/// `graphql!(SOURCE, name = value, …)`, or modules:
/// `graphql! { pub mod name = SOURCE; mod other = SOURCE }`.
enum Input {
    Expression {
        source: Source,
        variables: Vec<(Ident, Value)>,
    },
    Modules(Vec<Module>),
}

struct Module {
    attributes: Vec<Attribute>,
    visibility: Visibility,
    name: Ident,
    source: Source,
}

impl Parse for Input {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut attributes = input.call(Attribute::parse_outer)?;
        let mut visibility: Visibility = input.parse()?;

        if input.peek(Token![mod]) {
            let mut modules = Vec::new();
            loop {
                input.parse::<Token![mod]>()?;
                let name: Ident = input.parse()?;
                input.parse::<Token![=]>()?;
                let source: Source = input.parse()?;
                modules.push(Module {
                    attributes,
                    visibility,
                    name,
                    source,
                });
                if input.is_empty() {
                    break;
                }
                input.parse::<Token![;]>().map_err(|error| {
                    syn::Error::new(error.span(), "expected `;` after the module")
                })?;
                if input.is_empty() {
                    break;
                }
                attributes = input.call(Attribute::parse_outer)?;
                visibility = input.parse()?;
                if !input.peek(Token![mod]) {
                    return Err(input.error(
                        "expected another module, `mod name = \"…\";`, or the end of `graphql!`",
                    ));
                }
            }
            return Ok(Self::Modules(modules));
        }

        if let Some(attribute) = attributes.first() {
            return Err(syn::Error::new(
                attribute.span(),
                "attributes need the module form: `mod name = \"…\";`",
            ));
        }
        if !matches!(visibility, Visibility::Inherited) {
            return Err(syn::Error::new(
                visibility.span(),
                "a visibility needs the module form: `pub mod name = \"…\";`",
            ));
        }
        let source: Source = input.parse()?;
        let mut variables = Vec::new();
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let name: Ident = input.parse().map_err(|error| {
                syn::Error::new(error.span(), "expected a variable: `name = value`")
            })?;
            input.parse::<Token![=]>()?;
            variables.push((name, value::parse_value(input)?));
        }
        Ok(Self::Expression { source, variables })
    }
}

/// `input!(TypeName { field: value, … })`.
struct InputObject {
    name: Ident,
    object: Value,
}

impl Parse for InputObject {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        const FORM: &str =
            "expected an input object of the schema: `input!(TypeName { field: value, … })`";
        let name: Ident = input
            .parse()
            .map_err(|error| syn::Error::new(error.span(), FORM))?;
        if !input.peek(syn::token::Brace) {
            return Err(input.error(FORM));
        }
        let content;
        let braces = syn::braced!(content in input);
        let object = value::parse_object(&content, braces.span.join())?;
        if !input.is_empty() {
            return Err(input.error("expected the end of `input!`"));
        }
        Ok(Self { name, object })
    }
}

/// The text of an operation, where to point at it, and the file it was read from.
struct Text {
    query: LitStr,
    file: Option<PathBuf>,
}

impl Text {
    fn read(source: &Source) -> syn::Result<Self> {
        match source {
            Source::Literal(query) => Ok(Self {
                query: query.clone(),
                file: None,
            }),
            Source::File(path) => {
                let file = resolve(manifest_dir().as_deref(), Path::new(&path.value()));
                let text = std::fs::read_to_string(&file).map_err(|error| {
                    syn::Error::new(
                        path.span(),
                        format!(
                            "Cannot read `{}`, relative to the Cargo.toml of the crate: {error}",
                            path.value()
                        ),
                    )
                })?;
                Ok(Self {
                    query: LitStr::new(&text, path.span()),
                    file: Some(file),
                })
            }
        }
    }

    fn span(&self) -> Span {
        self.query.span()
    }

    /// Makes the compiler rebuild the crate when the file changes.
    fn tracking(&self) -> Option<TokenStream> {
        let file = self.file.as_ref()?.to_string_lossy().to_string();
        Some(quote!(
            const _: &str = ::core::include_str!(#file);
        ))
    }
}

fn manifest_dir() -> Option<PathBuf> {
    std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from)
}

fn resolve(manifest_dir: Option<&Path>, path: &Path) -> PathBuf {
    match manifest_dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path.to_path_buf(),
    }
}

/// Parsed schema files by path, with the modification time they were read at.
type SchemaCache = HashMap<PathBuf, (Option<SystemTime>, Arc<Schema>)>;

/// The schema operations are checked against, and its file.
struct LoadedSchema {
    path: PathBuf,
    schema: Arc<Schema>,
}

impl LoadedSchema {
    /// The schema of the crate being compiled.
    fn load() -> Result<Self, String> {
        // The variable is for the crates being built, not for their
        // dependencies, which ship their own schema files.
        let primary = std::env::var_os("CARGO_PRIMARY_PACKAGE").is_some();
        let named = std::env::var_os(SCHEMA_ENV).filter(|_| primary);
        let (path, schema) = match schema_source(manifest_dir().as_deref(), named.as_deref())? {
            SchemaSource::File(path) => {
                let schema = read_schema(&path).map_err(|error| {
                    format!(
                        "`{}`, named by {SCHEMA_ENV}: {error}. Download it again with \
                         `shopify-api schema download --out <file>`.",
                        path.display()
                    )
                })?;
                (path, schema)
            }
            SchemaSource::Project { dir, path, pinned } => {
                let read = |path: &Path| {
                    read_schema(path).map_err(|error| format!("`{}`: {error}", path.display()))
                };
                let schema =
                    project_schema(&dir, pinned.as_deref(), primary, SystemTime::now(), read)?;
                (path, schema)
            }
        };
        if !is_supported_api_version(&schema.api_version) {
            return Err(format!(
                "`{}` is the schema of `{}`, a version shopify_api does not call (from {MINIMUM_API_VERSION}, or `unstable`): \
                 declare another one in Cargo.toml:\n\n{}",
                path.display(),
                schema.api_version,
                metadata_snippet(&latest_api_version(SystemTime::now()))
            ));
        }
        Ok(Self { schema, path })
    }

    /// The schema of `shopify_api`'s own operations.
    fn internal() -> Result<Self, String> {
        let path = resolve(manifest_dir().as_deref(), Path::new(INTERNAL_SCHEMA_PATH));
        let schema =
            read_schema(&path).map_err(|error| format!("`{}`: {error}", path.display()))?;
        Ok(Self { schema, path })
    }

    fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Makes the compiler rebuild the crate when what the expansion depends on changes.
    fn tracking(&self) -> TokenStream {
        // One operation of the crate is enough to track a file, which may be large.
        static TRACKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
        let mut tracked = TRACKED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The manifest declares the API version, the schema file holds it.
        let manifest = manifest_dir().map(|dir| dir.join("Cargo.toml"));
        let files = [manifest, Some(self.path.clone())]
            .into_iter()
            .flatten()
            .filter(|path| path.is_file());
        let files: Vec<String> = files
            .filter(|path| {
                let new = !tracked.contains(path);
                if new {
                    tracked.push(path.clone());
                }
                new
            })
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        quote! {
            const _: ::core::option::Option<&str> = ::core::option_env!(#SCHEMA_ENV);
            #(const _: &[u8] = ::core::include_bytes!(#files);)*
        }
    }

    /// A refusal of a literal value, at its token.
    fn literal_error(&self, error: literal::Error) -> syn::Error {
        syn::Error::new(
            error.span,
            format!(
                "{}\n(checked against the Shopify Admin API {})",
                error.message,
                self.schema().api_version
            ),
        )
    }

    /// Checks an operation, pointing a refusal at its text.
    fn check(&self, text: &Text) -> syn::Result<Operation> {
        analyze(self.schema(), &text.query.value()).map_err(|error| {
            let file = match &text.file {
                Some(file) => {
                    let relative = manifest_dir()
                        .and_then(|dir| file.strip_prefix(dir).ok().map(Path::to_path_buf));
                    format!(" in {}", relative.as_deref().unwrap_or(file).display())
                }
                None => String::new(),
            };
            syn::Error::new(
                text.span(),
                format!(
                    "{error}{file}\n(checked against the Shopify Admin API {})",
                    self.schema().api_version
                ),
            )
        })
    }
}

/// Reads a schema file. One compilation expands many operations: each file is
/// parsed once, and again when it changes. The error does not name the file.
fn read_schema(file: &Path) -> Result<Arc<Schema>, String> {
    static CACHE: Mutex<Option<SchemaCache>> = Mutex::new(None);
    let modified = std::fs::metadata(file)
        .and_then(|metadata| metadata.modified())
        .ok();
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((cached, schema)) = cache.get(file)
        && *cached == modified
    {
        return Ok(schema.clone());
    }
    let json = std::fs::read_to_string(file).map_err(|error| format!("cannot read it: {error}"))?;
    let schema = Schema::from_json(&json)
        .map(Arc::new)
        .map_err(|error| error.to_string())?;
    cache.insert(file.to_path_buf(), (modified, schema.clone()));
    Ok(schema)
}

/// One warning per deprecated thing the operation uses: the use of a
/// `#[deprecated]` constant, at the operation, whose note says what and why.
fn deprecation_warnings(schema: &Schema, operation: &Operation, text: &Text) -> TokenStream {
    let warnings = operation.deprecations.iter().map(|deprecation| {
        let (line, column) = deprecation.position;
        let note = format!(
            "`{}` is deprecated in the Shopify Admin API {}: {} (line {line}, column {column} of the operation)",
            deprecation.coordinate, schema.api_version, deprecation.reason
        );
        deprecation_warning(text.span(), &note)
    });
    quote!(#(#warnings)*)
}

fn kind(operation: &Operation, krate: &TokenStream) -> TokenStream {
    match operation.kind {
        OperationKind::Query => quote!(#krate::OperationKind::Query),
        OperationKind::Mutation => quote!(#krate::OperationKind::Mutation),
    }
}

/// `Operation::<Data>::new(…)` for the checked operation.
fn constructor(
    schema: &Schema,
    operation: &Operation,
    query: &TokenStream,
    krate: &TokenStream,
) -> TokenStream {
    let data = format_ident!("{}", DATA_TYPE);
    let kind = kind(operation, krate);
    let api_version = &schema.api_version;
    let name = match &operation.name {
        Some(name) => quote!(::core::option::Option::Some(#name)),
        None => quote!(::core::option::Option::None),
    };
    quote!(#krate::Operation::<#data>::__checked(#query, #kind, #name, #api_version))
}

/// `graphql!("…", name = value)`: the operation as a value.
fn expand_expression(
    schema: &LoadedSchema,
    source: &Source,
    variables: &[(Ident, Value)],
) -> syn::Result<TokenStream> {
    let krate = quote!(::shopify_api);
    let text = Text::read(source)?;
    let operation = schema.check(&text)?;

    // Each value with the variable it is for.
    let mut given: Vec<(&Variable, &Value)> = Vec::new();
    for (name, value) in variables {
        let Some(declared) = operation
            .variables
            .iter()
            .find(|variable| name == &variable.name)
        else {
            let declared: Vec<String> = operation
                .variables
                .iter()
                .map(|variable| format!("${}", variable.name))
                .collect();
            let known = if declared.is_empty() {
                "it declares no variables".to_string()
            } else {
                format!("it declares {}", declared.join(", "))
            };
            return Err(syn::Error::new(
                name.span(),
                format!("The operation has no variable `${name}`: {known}"),
            ));
        };
        if given
            .iter()
            .any(|(earlier, _)| earlier.name == declared.name)
        {
            return Err(syn::Error::new(
                name.span(),
                format!("Variable `${name}` is given twice"),
            ));
        }
        given.push((declared, value));
    }
    if let Some(missing) = operation.variables.iter().find(|variable| {
        variable.is_required()
            && !given
                .iter()
                .any(|(declared, _)| declared.name == variable.name)
    }) {
        return Err(syn::Error::new(
            text.span(),
            format!(
                "Variable `${}: {}` needs a value: add `{} = …` after the operation",
                missing.name, missing.ty, missing.name
            ),
        ));
    }

    // The values are arguments of the setters, outside the block that
    // declares the types of the operation: they cannot name them.
    let mut literals = Literals::new(schema.schema(), krate.clone());
    let mut setters = Vec::new();
    for (declared, value) in &given {
        let key = &declared.name;
        match literals
            .variable(declared, value)
            .map_err(|error| schema.literal_error(error))?
        {
            VariableValue::Converted { ty, value } => {
                let tokens = literal::argument(&value);
                setters.push(quote_spanned!(value.span=> .variable::<#ty, _>(#key, #tokens)));
            }
            VariableValue::Literal(code) => {
                setters.push(quote!(.__literal_variable(#key, #code)));
            }
        }
    }
    let literal_warnings = literals.finish();

    let tracking = schema.tracking();
    let file = text.tracking();
    let warnings = deprecation_warnings(schema.schema(), &operation, &text);
    let types = data_types(&operation, &krate);
    let query = &text.query;
    let constructor = constructor(schema.schema(), &operation, &quote!(#query), &krate);
    Ok(quote! {
        ({
            #tracking
            #file
            #warnings
            #literal_warnings
            #types
            #constructor
        })
        #(#setters)*
    })
}

/// `graphql! { mod name = "…"; }`: the operation as a module that names its types.
fn expand_module(schema: &LoadedSchema, module: &Module) -> syn::Result<TokenStream> {
    let Module {
        attributes,
        visibility,
        name,
        source,
    } = module;
    let krate = quote!(::shopify_api);
    let text = Text::read(source)?;
    let operation = schema.check(&text)?;
    let data = format_ident!("{}", DATA_TYPE);
    let constructor = constructor(schema.schema(), &operation, &quote!(QUERY), &krate);
    let variables = if operation.variables.is_empty() {
        quote! {
            /// The operation, for `Shopify::execute`.
            pub fn operation() -> #krate::Operation<#data> {
                #constructor
            }
        }
    } else {
        variables_types(schema.schema(), &operation, &krate, &constructor)
    };

    let tracking = schema.tracking();
    let file = text.tracking();
    let warnings = deprecation_warnings(schema.schema(), &operation, &text);
    let types = data_types(&operation, &krate);
    let query = &text.query;
    let api_version = &schema.schema().api_version;
    let operation_name = match &operation.name {
        Some(name) => quote!(::core::option::Option::Some(#name)),
        None => quote!(::core::option::Option::None),
    };
    let kind = kind(&operation, &krate);
    Ok(quote! {
        #(#attributes)*
        #visibility mod #name {
            #tracking
            #file
            #warnings

            /// The text of the operation.
            pub const QUERY: &str = #query;
            /// The name of the operation, when it has one.
            pub const NAME: ::core::option::Option<&str> = #operation_name;
            /// A query or a mutation.
            pub const KIND: #krate::OperationKind = #kind;
            /// The Admin API version the operation was checked against.
            pub const API_VERSION: &str = #api_version;

            #types
            #variables
        }
    })
}

fn expand(input: Input, schema: Result<LoadedSchema, String>) -> syn::Result<TokenStream> {
    let schema = schema.map_err(|message| syn::Error::new(Span::call_site(), message))?;
    match input {
        Input::Expression { source, variables } => expand_expression(&schema, &source, &variables),
        Input::Modules(modules) => {
            let mut expanded = TokenStream::new();
            for module in &modules {
                expanded.extend(expand_module(&schema, module)?);
            }
            Ok(expanded)
        }
    }
}

/// `input!(TypeName { … })`: an `InputObject` of that type.
fn expand_input(
    input: InputObject,
    schema: Result<LoadedSchema, String>,
) -> syn::Result<TokenStream> {
    let schema = schema.map_err(|message| syn::Error::new(Span::call_site(), message))?;
    let Value::Object(object) = &input.object else {
        unreachable!("`input!` reads an object");
    };
    let mut literals = Literals::new(schema.schema(), quote!(::shopify_api));
    let value = literals
        .input_object(&input.name.to_string(), input.name.span(), object)
        .map_err(|error| schema.literal_error(error))?;
    let checks = literals.finish();
    let tracking = schema.tracking();
    Ok(quote! {{
        #tracking
        #checks
        #value
    }})
}

/// See `shopify_api::graphql!`.
#[proc_macro]
pub fn graphql(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as Input);
    expand(input, LoadedSchema::load())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// See `shopify_api::input!`.
#[proc_macro]
pub fn input(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as InputObject);
    expand_input(input, LoadedSchema::load())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `graphql!` for the operations of `shopify_api` itself, checked against its
/// `graphql/internal.schema.json`. Not for other crates.
#[doc(hidden)]
#[proc_macro]
pub fn __graphql_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as Input);
    expand(input, LoadedSchema::internal())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `input!` for `shopify_api` itself. Not for other crates.
#[doc(hidden)]
#[proc_macro]
pub fn __input_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as InputObject);
    expand_input(input, LoadedSchema::internal())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"{"format":2,"apiVersion":"2026-07","query":"QueryRoot","types":{
"QueryRoot":{"o":{"f":{"ping":"String"}}},
"String":"s"
}}"#;

    /// An empty directory removed when the test ends.
    struct Project(PathBuf);

    impl Project {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("shopify-api-macros-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, path: &str, content: &str) -> PathBuf {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, content).unwrap();
            file
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn schema_files_are_read_once_and_again_when_they_change() {
        let project = Project::new("read");
        let file = project.write("graphql/admin.schema.json", SCHEMA);

        let first = read_schema(&file).unwrap();
        assert_eq!(first.api_version, "2026-07");
        assert!(Arc::ptr_eq(&first, &read_schema(&file).unwrap()));

        // A later modification time, as after `shopify-api schema download`.
        std::fs::write(&file, SCHEMA.replace("2026-07", "2026-10")).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(read_schema(&file).unwrap().api_version, "2026-10");
    }

    #[test]
    fn a_malformed_schema_file_says_why() {
        let project = Project::new("malformed");
        let file = project.write("graphql/admin.schema.json", "{");

        let error = read_schema(&file).unwrap_err();
        assert!(error.starts_with("Invalid schema file: "), "{error}");

        let error = read_schema(&project.0.join("absent.json")).unwrap_err();
        assert!(error.starts_with("cannot read it: "), "{error}");
    }

    #[test]
    fn parses_both_forms_and_both_sources() {
        let expression: Input =
            syn::parse_str(r#""{ shop { name } }", first = 1, after = cursor,"#).unwrap();
        let Input::Expression {
            source: Source::Literal(query),
            variables,
        } = expression
        else {
            panic!("an expression with a literal");
        };
        assert_eq!(query.value(), "{ shop { name } }");
        let names: Vec<String> = variables.iter().map(|(name, _)| name.to_string()).collect();
        assert_eq!(names, ["first", "after"]);

        let expression: Input = syn::parse_str(r#"file("graphql/shop.graphql"), id = id"#).unwrap();
        let Input::Expression {
            source: Source::File(path),
            variables,
        } = expression
        else {
            panic!("an expression with a file");
        };
        assert_eq!(path.value(), "graphql/shop.graphql");
        assert_eq!(variables.len(), 1);

        let module: Input =
            syn::parse_str("#[doc = \"Docs.\"] pub mod shop_name = \"{ shop { name } }\";")
                .unwrap();
        let Input::Modules(modules) = module else {
            panic!("a module");
        };
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "shop_name");
        assert_eq!(modules[0].attributes.len(), 1);

        // Several modules, `;` between them and optional after the last.
        for text in [
            r#"#[doc = "A."] pub mod a = "{ shop { name } }"; pub(crate) mod b = file("b.graphql"); mod c = "{ shop { id } }""#,
            r#"#[doc = "A."] pub mod a = "{ shop { name } }"; pub(crate) mod b = file("b.graphql"); mod c = "{ shop { id } }";"#,
        ] {
            let Input::Modules(modules) = syn::parse_str::<Input>(text).unwrap() else {
                panic!("modules");
            };
            let names: Vec<String> = modules
                .iter()
                .map(|module| module.name.to_string())
                .collect();
            assert_eq!(names, ["a", "b", "c"]);
            assert_eq!(modules[0].attributes.len(), 1);
            assert!(modules[1].attributes.is_empty());
            assert!(matches!(modules[0].visibility, Visibility::Public(_)));
            assert!(matches!(modules[1].visibility, Visibility::Restricted(_)));
            assert!(matches!(modules[1].source, Source::File(_)));
            assert!(matches!(modules[2].visibility, Visibility::Inherited));
        }

        for (source, message) in [
            (
                "first = 1",
                "expected the GraphQL operation as a string literal, or `file(\"path.graphql\")`",
            ),
            (
                r#"path("x.graphql")"#,
                "expected the GraphQL operation as a string literal, or `file(\"path.graphql\")`",
            ),
            (r#"file("a", "b")"#, "`file(…)` takes one path"),
            (
                r#""{ shop { name } }", 1"#,
                "expected a variable: `name = value`",
            ),
            (
                r#"pub "{ shop { name } }""#,
                "a visibility needs the module form: `pub mod name = \"…\";`",
            ),
            (
                r#"mod shop = "{ shop { name } }"; 1"#,
                "expected another module, `mod name = \"…\";`, or the end of `graphql!`",
            ),
            (
                r#"mod a = "{ shop { name } }" mod b = "{ shop { id } }""#,
                "expected `;` after the module",
            ),
        ] {
            let error = syn::parse_str::<Input>(source)
                .err()
                .expect(source)
                .to_string();
            assert_eq!(error, message, "{source}");
        }
    }
}
