// #![allow(warnings)]
use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(PartialEq, Eq, Hash, Debug, serde::Deserialize)]
struct Input {
    description: Option<String>,
    #[serde(rename(deserialize = "deprecationMessage"))]
    deprecation_message: Option<String>,
    default: Option<String>,
    required: Option<bool>,
}

#[derive(PartialEq, Eq, Hash, Debug, serde::Deserialize)]
struct Output {
    description: Option<String>,
}

#[derive(PartialEq, Eq, Hash, Debug, serde::Deserialize)]
struct Branding {
    icon: Option<String>,
    color: Option<String>,
}

#[derive(PartialEq, Eq, Debug, serde::Deserialize)]
struct Manifest {
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    branding: Option<Branding>,

    #[serde(default)]
    inputs: HashMap<String, Input>,
    #[serde(default)]
    outputs: HashMap<String, Output>,
}

fn parse_action_yml(path: impl AsRef<Path>) -> Manifest {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path.as_ref())
        .unwrap();
    let reader = std::io::BufReader::new(file);
    serde_yaml::from_reader(reader).unwrap()
}

fn resolve_path(path: impl AsRef<Path>) -> PathBuf {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    if root.join(path.as_ref()).exists() {
        root.join(path.as_ref())
    } else {
        root.join("src/").join(&path.as_ref())
    }
}

fn replace_invalid_identifier_chars(s: &str) -> String {
    s.strip_prefix('$')
        .unwrap_or(s)
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "_")
}

fn replace_numeric_start(s: &str) -> String {
    if s.chars().next().map(|c| c.is_numeric()).unwrap_or(false) {
        format!("_{}", s)
    } else {
        s.to_string()
    }
}

fn remove_excess_underscores(s: &str) -> String {
    let mut result = String::new();
    let mut char_iter = s.chars().peekable();

    while let Some(c) = char_iter.next() {
        let next_c = char_iter.peek();
        if c != '_' || !matches!(next_c, Some('_')) {
            result.push(c);
        }
    }

    result
}

fn str_to_ident(s: &str) -> syn::Ident {
    if s.is_empty() {
        return syn::Ident::new("empty_", Span::call_site());
    }

    if s.chars().all(|c| c == '_') {
        return syn::Ident::new("underscore_", Span::call_site());
    }

    let s = replace_invalid_identifier_chars(s);
    let s = replace_numeric_start(&s);
    let s = remove_excess_underscores(&s);

    if s.is_empty() {
        return syn::Ident::new("invalid_", Span::call_site());
    }

    let keywords = [
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
        "where", "while", "abstract", "become", "box", "do", "final", "macro", "override", "priv",
        "typeof", "unsized", "virtual", "yield", "async", "await", "try",
    ];
    if keywords.iter().any(|&keyword| keyword == s) {
        return syn::Ident::new(&format!("{}_", s), Span::call_site());
    }

    syn::Ident::new(&s, Span::call_site())
}

fn format_option<T: quote::ToTokens>(value: &Option<T>) -> TokenStream {
    match value {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    }
}

fn get_attribute(attr: &syn::Attribute) -> String {
    match &attr.parse_meta().unwrap() {
        syn::Meta::NameValue(syn::MetaNameValue { lit, .. }) => match lit {
            syn::Lit::Str(s) => s.value(),
            _ => panic!("action attribute must be a string"),
        },
        _ => panic!("action attribute must be of the form `action = \"...\"`"),
    }
}

fn parse_derive(ast: &syn::DeriveInput) -> (&syn::Ident, &syn::Generics, PathBuf) {
    let name = &ast.ident;
    let generics = &ast.generics;

    let manifests: Vec<_> = ast
        .attrs
        .iter()
        .filter(|attr| attr.path.is_ident("action"))
        .map(get_attribute)
        .map(resolve_path)
        .collect();

    let manifest = manifests.into_iter().next().expect("a path to an action manifest (action.yml) file needs to be provided with the #[action = \"PATH\"] attribute");
    (name, generics, manifest)
}

#[proc_macro_derive(Action, attributes(action))]
pub fn action_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast: syn::DeriveInput = syn::parse2(input.into()).unwrap();
    let (struct_name, generics, manifest) = parse_derive(&ast);

    // match &ast.data {
    //     syn::Data::Struct(syn::DataStruct { fields, .. }) => {
    //         if !fields.is_empty() {
    //             panic!("Action can only be derived for empty structs")
    //         }
    //     },
    //     _ => panic!("Action can only be derived for structs"),
    // }

    let manifest = parse_action_yml(manifest);
    // dbg!(&manifest);

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let derived_methods: TokenStream = manifest
        .inputs
        .iter()
        .map(|(name, _input)| {
            let fn_name = str_to_ident(&name);
            quote! {
                pub fn #fn_name<T>() -> Result<Option<T>, <T as ::actions::ParseInput>::Error>
                where T: ::actions::ParseInput {
                    ::actions::get_input::<T>(#name)
                }
            }
        })
        .collect();

    let input_enum = quote! { pub enum Test {} };

    let inputs: Vec<_> = manifest
        .inputs
        .iter()
        .map(|(name, input)| {
            let description = format_option(&input.description);
            let deprecation_message = format_option(&input.deprecation_message);
            let r#default = format_option(&input.default);
            let required = format_option(&input.required);
            quote! {
                (#name, ::actions::Input {
                    description: #description,
                    deprecation_message: #deprecation_message,
                    default: #r#default,
                    required: #required,
                })
            }
        })
        .collect();
    // eprintln!("{}", pretty_print(&quote! { vec![#(#inputs,)*]; }));

    let parse_impl = quote! {
        #[allow(clippy::all)]
        impl #impl_generics ::actions::Parse for #struct_name #ty_generics #where_clause {
            fn parse<E: ::actions::ReadEnv>(env: &E) -> std::collections::HashMap<String, Option<String>> {
                Self::inputs().iter().map(|(name, input)| {
                    let value = ::actions::get_input_from::<String>(env, name);
                    let default = input.default.map(|s| s.to_string());
                    (name.to_string(), value.unwrap().or(default))
                }).collect()
            }
        }
    };

    let Manifest {
        name,
        description,
        author,
        ..
    } = manifest;

    let input_impl = quote! {
        #[allow(clippy::all)]
        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Input names of this action.
            // pub fn input_names() -> &'static [&'static str] {
            //     static names: &'static [&'static str] = &[#(#input_names,)*];
            //     &names
            // }

            /// Inputs of this action.
            pub fn inputs() -> ::std::collections::HashMap<
                &'static str, ::actions::Input<'static>
            > {
                static inputs: &'static [(&'static str, ::actions::Input<'static>)] = &[
                    #(#inputs,)*
                ];
                inputs.iter().cloned().collect()
                // vec![].into_iter().collect()
                // vec![#(#fields,)*].into_iter().collect()
            }

            pub fn description() -> &'static str {
                #description
            }

            pub fn name() -> &'static str {
                #name
            }

            pub fn author() -> &'static str {
                #author
            }

            #derived_methods
        }
    };
    let tokens = quote! {
        #input_enum
        #parse_impl
        #input_impl
    };
    eprintln!("{}", pretty_print(&tokens));
    tokens.into()
}

#[allow(dead_code)]
fn pretty_print(tokens: &TokenStream) -> String {
    let _file = syn::parse_file(&tokens.to_string()).unwrap();
    // TODO: this will not work until prettyplease updates to syn 2+
    // prettyplease::unparse(&file);
    tokens.to_string()
}
