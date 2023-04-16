#![allow(warnings)]
// use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream};
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
struct ActionAttributes {
    // root: Option<String>,
    // manifest_path: syn::LitStr,
    manifest_path: PathBuf,
}

impl syn::parse::Parse for ActionAttributes {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let manifest_path: syn::LitStr = input.parse()?;
        Ok(Self {
            manifest_path: resolve_path(manifest_path.value()),
        })
    }
}

fn get_attribute(attr: &syn::Attribute) -> String {
    // syn::NestedMeta::Lit(lit) => match lit {
    //             syn::Lit::Str(s) => {

    let meta = attr.parse_meta().unwrap();
    // println!("{:?}", quote! { #meta });
    // "".into()
    match &meta {
        syn::Meta::NameValue(syn::MetaNameValue { lit, .. }) => match lit {
            // syn::Meta::NameValue(name_value) => match name_value {
            syn::Lit::Str(s) => s.value(),
            // syn::Expr::Lit(syn::ExprLit {
            //     lit: syn::Lit::Str(s),
            //     ..
            // }) => s.value(),
            _ => panic!("action attribute must be a string"),
        },
        _ => panic!("action attribute must be of the form `action = \"...\"`"),
    }
}

fn parse_derive(ast: syn::DeriveInput) -> (syn::Ident, syn::Generics, PathBuf) {
    let name = ast.ident;
    let generics = ast.generics;

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
    // let action: ActionAttributes = syn::parse2(attrs.into()).unwrap();
    let ast: syn::DeriveInput = syn::parse2(input.into()).unwrap();
    let (name, generics, manifest) = parse_derive(ast);
    let manifest = parse_action_yml(manifest);
    println!("name: {:?}", name);
    // println!("generics: {:?}", generics);
    println!("manifest: {:?}", manifest);
    // let tokens: TokenStream = quote! {}.into();

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let derived_methods: TokenStream = manifest
        .inputs
        .into_iter()
        .map(|(name, input)| {
            let fn_name = str_to_ident(&name);
            quote! {
                pub fn #fn_name<T>() -> Result<Option<T>, <T as ::actions::ParseInput>::Error>
                where T: ::actions::ParseInput {
                    ::actions::get_input::<T>(#name)
                }
            }
        })
        .collect();

    let description = manifest.description;

    let input_impl = quote! {
        #[allow(clippy::all)]
        impl #impl_generics #name #ty_generics #where_clause {
            pub fn description() -> &'static str {
                #description
            }
            #derived_methods
        }
    };

    // let input_impl = quote! {
    //     #[allow(clippy::all)]
    //     impl #impl_generics ::pest::Parser<Rule> for #name #ty_generics #where_clause {
    //         fn parse<'i>(
    //             rule: Rule,
    //             input: &'i str
    //         ) -> #result<
    //             ::pest::iterators::Pairs<'i, Rule>,
    //             ::pest::error::Error<Rule>
    //         > {
    //             mod rules {
    //                 #![allow(clippy::upper_case_acronyms)]
    //                 pub mod hidden {
    //                     use super::super::Rule;
    //                     #skip
    //                 }
    //
    //                 pub mod visible {
    //                     use super::super::Rule;
    //                     #( #rules )*
    //                 }
    //
    //                 pub use self::visible::*;
    //             }
    //
    //             ::pest::state(input, |state| {
    //                 match rule {
    //                     #patterns
    //                 }
    //             })
    //         }
    //     }
    // };
    // eprintln!("TOKENS: {}", tokens);
    // eprintln!("{}", input_impl.into())
    eprintln!("{}", &input_impl.to_string());
    input_impl.into()
}

#[proc_macro_attribute]
pub fn action(
    attrs: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    eprintln!("{:#?}", attrs);
    // let action = syn::parse_macro_input!(attrs as ActionAttributes);
    let action: ActionAttributes = syn::parse2(attrs.into()).unwrap();
    eprintln!("{}", action.manifest_path.display());

    // let attrs = syn::parse_macro_input!(attrs as syn::AttributeArgs);
    let ast: syn::DeriveInput = syn::parse2(input.into()).unwrap();
    // let ast = syn::parse_macro_input!(input as syn::ItemStruct);

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

    // assert!(matches!(ast.data, syn::Data::Ident { .. }));
    assert!(matches!(
        ast.data,
        syn::Data::Struct(syn::DataStruct { .. })
    ));
    // let override_fields = match ast.data {
    //     // syn::Data::Struct(syn::DataStruct {
    //     //     fields: syn::Fields::Named(syn::FieldsNamed { ref named, .. }),
    //     //     ..
    //     // }) => named,
    //     syn::Data::Struct(syn::ItemStruct {
    //         // fields: syn::Fields::Named(syn::FieldsNamed { ref named, .. }),
    //         ..
    //     }) => named,
    //
    //     other => unimplemented!("action macro can only be used on structs, got {:?}", other),
    // };

    // let builder_fields = override_fields.iter().map(|f| {
    //     let name = &f.ident;
    //     let ty = &f.ty;
    //     for attr in &f.attrs {
    //         // if attr.path().is_ident("repr") {}
    //     }
    //     // eprintln!("field {:?}: {:#?}", name, ty.to_token_stream());
    //     quote! { pub #name: #ty }
    // });

    let derived_fields = fields.iter().map(|(name, ty)| {
        quote! { pub #name: #ty }
    });

    let tokens = quote! {
        pub struct #name {
            #(#derived_fields,)*
        }
    };
    // let tokens = quote! {
    //     #struct
    // };

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
