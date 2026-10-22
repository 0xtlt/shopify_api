//! The `graphql!` macro of `shopify_api`. Use it through `shopify_api::graphql!`,
//! which re-exports and documents it.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use shopify_api_graphql::codegen::{
    DATA_TYPE, VARIABLES_TYPE, data_types, variable_type, variables_types,
};
use shopify_api_graphql::{
    MINIMUM_API_VERSION, Operation, OperationKind, PROJECT_SCHEMA_PATH, SCHEMA_ENV, Schema,
    Variable, analyze, is_supported_api_version,
};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Attribute, Expr, Ident, LitStr, Token, Visibility};

/// The package whose own operations are always checked against the bundled
/// schema: the schema a crate that depends on it selects is not its own.
const RUNTIME_PACKAGE: &str = "shopify_api";

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

/// `graphql!(SOURCE, name = value, …)` or `graphql! { pub mod name = SOURCE; }`.
enum Input {
    Expression {
        source: Source,
        variables: Vec<(Ident, Expr)>,
    },
    Module {
        attributes: Vec<Attribute>,
        visibility: Visibility,
        name: Ident,
        source: Source,
    },
}

impl Parse for Input {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attributes = input.call(Attribute::parse_outer)?;
        let visibility: Visibility = input.parse()?;

        if input.peek(Token![mod]) {
            input.parse::<Token![mod]>()?;
            let name: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let source: Source = input.parse()?;
            input.parse::<Option<Token![;]>>()?;
            if !input.is_empty() {
                return Err(input.error("expected the end of the `graphql!` module"));
            }
            return Ok(Self::Module {
                attributes,
                visibility,
                name,
                source,
            });
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
            variables.push((name, input.parse()?));
        }
        Ok(Self::Expression { source, variables })
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

/// The schema operations are checked against.
enum LoadedSchema {
    /// The one bundled with `shopify_api_graphql`.
    Bundled,
    /// The one of the crate being compiled.
    File { path: PathBuf, schema: Arc<Schema> },
}

impl LoadedSchema {
    /// The schema of the crate being compiled, else the bundled one.
    fn load() -> Result<Self, String> {
        if std::env::var("CARGO_PKG_NAME").is_ok_and(|name| name == RUNTIME_PACKAGE) {
            return Ok(Self::Bundled);
        }
        // The variable is for the crates being built, not for their
        // dependencies, which ship their own schema files.
        let primary = std::env::var_os("CARGO_PRIMARY_PACKAGE").is_some();
        let named = std::env::var_os(SCHEMA_ENV).filter(|_| primary);
        Ok(
            match schema_file(manifest_dir().as_deref(), named.as_deref())? {
                Some(path) => {
                    let schema = read_schema(&path)?;
                    if !is_supported_api_version(&schema.api_version) {
                        return Err(format!(
                            "`{}` is the schema of `{}`, a version shopify_api does not call (from {MINIMUM_API_VERSION}, or `unstable`): \
                             download another one with `shopify-api schema download --api-version …`",
                            path.display(),
                            schema.api_version
                        ));
                    }
                    Self::File { schema, path }
                }
                None => Self::Bundled,
            },
        )
    }

    fn schema(&self) -> &Schema {
        match self {
            Self::Bundled => Schema::bundled(),
            Self::File { schema, .. } => schema,
        }
    }

    /// Makes the compiler rebuild the crate when what the expansion depends on changes.
    fn tracking(&self) -> TokenStream {
        // One operation of the crate is enough to track the file, which is large.
        static TRACKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
        let file = match self {
            Self::File { path, .. } => {
                let mut tracked = TRACKED
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (!tracked.contains(path)).then(|| {
                    tracked.push(path.clone());
                    let path = path.to_string_lossy().to_string();
                    quote!(
                        const _: &[u8] = ::core::include_bytes!(#path);
                    )
                })
            }
            Self::Bundled => None,
        };
        quote! {
            const _: ::core::option::Option<&str> = ::core::option_env!(#SCHEMA_ENV);
            #file
        }
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

/// The schema file of the crate being compiled, if it has one: the file named
/// by `SHOPIFY_GRAPHQL_SCHEMA`, else `graphql/admin.schema.json` next to its
/// `Cargo.toml` when that file exists.
fn schema_file(
    manifest_dir: Option<&Path>,
    named: Option<&OsStr>,
) -> Result<Option<PathBuf>, String> {
    match named.filter(|value| !value.is_empty()) {
        Some(named) => {
            let path = resolve(manifest_dir, Path::new(named));
            if !path.is_file() {
                return Err(format!(
                    "{SCHEMA_ENV} names `{}`, which is not a file",
                    path.display()
                ));
            }
            Ok(Some(path))
        }
        None => Ok(Some(resolve(manifest_dir, Path::new(PROJECT_SCHEMA_PATH)))
            .filter(|path| path.is_file())),
    }
}

/// Reads a schema file. One compilation expands many operations: each file is
/// parsed once, and again when it changes.
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
    let json = std::fs::read_to_string(file)
        .map_err(|error| format!("Cannot read the schema `{}`: {error}", file.display()))?;
    let schema = Schema::from_json(&json).map(Arc::new).map_err(|error| {
        format!(
            "`{}`: {error}. Run `shopify-api schema download` to download it again.",
            file.display()
        )
    })?;
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
        quote_spanned! {text.span()=>
            const _: () = {
                #[deprecated(note = #note)]
                const SHOPIFY_DEPRECATION: () = ();
                SHOPIFY_DEPRECATION
            };
        }
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
    variables: &[(Ident, Expr)],
) -> syn::Result<TokenStream> {
    let krate = quote!(::shopify_api);
    let text = Text::read(source)?;
    let operation = schema.check(&text)?;

    // Each value with the variable it is for.
    let mut given: Vec<(&Variable, &Expr)> = Vec::new();
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

    // Evaluated before the generated types are in scope, so that the
    // values cannot name them by accident.
    let locals: Vec<Ident> = (0..given.len())
        .map(|index| {
            Ident::new(
                &format!("__shopify_api_variable_{index}"),
                Span::mixed_site(),
            )
        })
        .collect();
    let values = given.iter().map(|(_, value)| value);
    let setters = given.iter().zip(&locals).map(|((declared, value), local)| {
        let key = &declared.name;
        match variable_type(declared, &krate) {
            Some(ty) => quote_spanned!(value.span()=> .variable::<#ty, _>(#key, #local)),
            None => quote_spanned!(value.span()=> .json_variable(#key, #local)),
        }
    });

    let tracking = schema.tracking();
    let file = text.tracking();
    let warnings = deprecation_warnings(schema.schema(), &operation, &text);
    let types = data_types(&operation, &krate);
    let query = &text.query;
    let constructor = constructor(schema.schema(), &operation, &quote!(#query), &krate);
    Ok(quote! {{
        #(let #locals = #values;)*
        {
            #tracking
            #file
            #warnings
            #types
            #constructor
                #(#setters)*
        }
    }})
}

/// `graphql! { mod name = "…"; }`: the operation as a module that names its types.
fn expand_module(
    schema: &LoadedSchema,
    attributes: &[Attribute],
    visibility: &Visibility,
    name: &Ident,
    source: &Source,
) -> syn::Result<TokenStream> {
    let krate = quote!(::shopify_api);
    let text = Text::read(source)?;
    let operation = schema.check(&text)?;
    let data = format_ident!("{}", DATA_TYPE);
    let constructor = constructor(schema.schema(), &operation, &quote!(QUERY), &krate);
    let (variables, function) = if operation.variables.is_empty() {
        let function = quote! {
            /// The operation, for `Shopify::execute`.
            pub fn operation() -> #krate::Operation<#data> {
                #constructor
            }
        };
        (TokenStream::new(), function)
    } else {
        let variables = format_ident!("{}", VARIABLES_TYPE);
        let function = quote! {
            /// The operation with its variables, for `Shopify::execute`.
            pub fn operation(variables: #variables) -> #krate::Operation<#data> {
                #constructor.variables(variables)
            }
        };
        (
            variables_types(schema.schema(), &operation, &krate),
            function,
        )
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
            #function
        }
    })
}

fn expand(input: Input) -> syn::Result<TokenStream> {
    let schema =
        LoadedSchema::load().map_err(|message| syn::Error::new(Span::call_site(), message))?;
    match input {
        Input::Expression { source, variables } => expand_expression(&schema, &source, &variables),
        Input::Module {
            attributes,
            visibility,
            name,
            source,
        } => expand_module(&schema, &attributes, &visibility, &name, &source),
    }
}

/// See `shopify_api::graphql!`.
#[proc_macro]
pub fn graphql(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as Input);
    expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"{"format":1,"apiVersion":"2026-07","query":"QueryRoot","types":{
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
    fn a_crate_without_a_schema_file_uses_the_bundled_schema() {
        let project = Project::new("bundled");
        assert_eq!(schema_file(Some(&project.0), None), Ok(None));
        assert_eq!(
            schema_file(Some(&project.0), Some(OsStr::new(""))),
            Ok(None)
        );
        assert_eq!(schema_file(None, None), Ok(None));
    }

    #[test]
    fn the_crate_file_and_the_environment_select_another_schema() {
        let project = Project::new("selected");
        let downloaded = project.write("graphql/admin.schema.json", SCHEMA);
        let other = project.write("schemas/unstable.json", SCHEMA);

        assert_eq!(schema_file(Some(&project.0), None), Ok(Some(downloaded)));
        // The environment wins, relative to the manifest or absolute.
        let named = OsStr::new("schemas/unstable.json");
        assert_eq!(
            schema_file(Some(&project.0), Some(named)),
            Ok(Some(other.clone()))
        );
        assert_eq!(schema_file(None, Some(other.as_os_str())), Ok(Some(other)));

        let missing = project.0.join("missing.json");
        assert_eq!(
            schema_file(Some(&project.0), Some(OsStr::new("missing.json"))),
            Err(format!(
                "SHOPIFY_GRAPHQL_SCHEMA names `{}`, which is not a file",
                missing.display()
            ))
        );
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
    fn a_malformed_schema_file_says_how_to_restore_it() {
        let project = Project::new("malformed");
        let file = project.write("graphql/admin.schema.json", "{");

        let error = read_schema(&file).unwrap_err();
        assert!(
            error.starts_with(&format!("`{}`: Invalid schema file: ", file.display())),
            "{error}"
        );
        assert!(
            error.ends_with(". Run `shopify-api schema download` to download it again."),
            "{error}"
        );

        let error = read_schema(&project.0.join("absent.json")).unwrap_err();
        assert!(error.starts_with("Cannot read the schema `"), "{error}");
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
        let Input::Module {
            attributes, name, ..
        } = module
        else {
            panic!("a module");
        };
        assert_eq!(name, "shop_name");
        assert_eq!(attributes.len(), 1);

        let module: Input =
            syn::parse_str(r#"pub(crate) mod shop_name = file("graphql/shop.graphql");"#).unwrap();
        assert!(matches!(
            module,
            Input::Module {
                source: Source::File(_),
                ..
            }
        ));

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
                "expected the end of the `graphql!` module",
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
