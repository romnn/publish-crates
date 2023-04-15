#![allow(warnings)]
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn impl_hello_macro(ast: &syn::DeriveInput) -> TokenStream {
    let name = &ast.ident;
    // dbg!(ast);
    // dbg!("name");
    let gen = quote! {
        // pub strucft
    };
    // let gen = quote! {
    //     impl HelloMacro for #name {
    //         fn hello_macro() {
    //             println!("Hello, Macro! My name is {}!", stringify!(#name));
    //         }
    //     }
    // };
    gen.into()
}

// if attr.path.is_ident("holding_register") {
//     let lit: syn::LitInt = attr.parse_args()?;
//     let n: u16 = lit.base10_parse()?;
//     ...
// }

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

#[proc_macro_derive(Action)]
pub fn action_derive(input: TokenStream) -> TokenStream {
    let tokens: TokenStream = quote! {}.into();
    // eprintln!("TOKENS: {}", tokens);
    tokens
}

fn resolve_path(path: impl AsRef<Path>) -> PathBuf {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    if root.join(path.as_ref()).exists() {
        root.join(path.as_ref())
    } else {
        root.join("src/").join(&path.as_ref())
    }
}

fn parse_action_yml(path: impl AsRef<Path>) -> Manifest {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path.as_ref())
        .unwrap();
    let mut reader = std::io::BufReader::new(file);
    serde_yaml::from_reader(reader).unwrap()
}

use proc_macro2::Span;

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

#[derive()]
struct Action {
    // root: Option<String>,
    // manifest_path: syn::LitStr,
    manifest_path: PathBuf,
}

impl syn::parse::Parse for Action {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let manifest_path: syn::LitStr = input.parse()?;
        Ok(Action {
            manifest_path: resolve_path(manifest_path.value()),
        })
    }
}

#[proc_macro_attribute]
pub fn action(attrs: TokenStream, input: TokenStream) -> TokenStream {
    eprintln!("{:#?}", attrs);
    let action = syn::parse_macro_input!(attrs as Action);
    eprintln!("{}", action.manifest_path.display());
    // let attrs = syn::parse_macro_input!(attrs as syn::AttributeArgs);
    let ast = syn::parse_macro_input!(input as syn::DeriveInput);

    // let mut manifest = None;
    // for attr in attrs {
    //     match attr {
    //         syn::NestedMeta::Lit(lit) => match lit {
    //             syn::Lit::Str(s) => {
    //                 let path = resolve_path(s.value());
    //                 // eprintln!("manifest: {:#?}", path);
    //                 manifest = Some(parse_action_yml(path));
    //                 // eprintln!("manifest: {:#?}", manifest);
    //             }
    //             _ => {}
    //         },
    //         syn::NestedMeta::Meta(syn::Meta::Path(p)) => {
    //             eprintln!("path: {:#?}", p.to_token_stream());
    //         }
    //         _ => {}
    //     }
    // }

    let manifest = parse_action_yml(action.manifest_path);
    // let manifest: Manifest = manifest.unwrap();

    // let string_typ = syn::Path::parse_mod_style(quote!{"String"}).unwrap();
    // check: https://github.com/Marwes/schemafy/blob/master/schemafy_lib/src/lib.rs
    let fields: Vec<_> = manifest
        .inputs
        .into_iter()
        .map(|(name, input)| {
            // let name = syn::Ident::new(&name, quote::Span::call_site());
            // (name, syn::Type::Path(syn::TypePath::from(string_typ))),
            (
                str_to_ident(&name),
                // syn::Type::Path(syn::TypePath::"empty_", Span::call_site());
                str_to_ident("String"),
                // quote::format_ident!("String"),
            )
            // (name, "todo")
        })
        .collect();

    // // get fields from manifest
    // for input in manifest.inputs {
    //     fields
    // }

    let name = ast.ident;
    // eprintln!("{}", name);

    let override_fields = match ast.data {
        syn::Data::Struct(syn::DataStruct {
            fields: syn::Fields::Named(syn::FieldsNamed { ref named, .. }),
            ..
        }) => named,
        _ => unimplemented!("action macro can only be used on structs"),
    };

    let builder_fields = override_fields.iter().map(|f| {
        let name = &f.ident;
        let ty = &f.ty;
        for attr in &f.attrs {
            // if attr.path().is_ident("repr") {}
        }
        // eprintln!("field {:?}: {:#?}", name, ty.to_token_stream());
        quote! { pub #name: #ty }
    });

    let derived_fields = fields.iter().map(|(name, ty)| {
        quote! { pub #name: #ty }
    });

    let tokens = quote! {
        pub struct #name {
            #(#derived_fields,)*
        }
    };
    // eprintln!("{:?}", &tokens);
    eprintln!("{}", &tokens.to_string());
    tokens.into()
    // dbg!(&attr);
    // dbg!(&input);

    // quote! { struct Test {} }.into()
    // Build the trait implementation
    // impl_hello_macro(&ast)
}

#[cfg(test)]
mod tests {
    use super::*;
}
