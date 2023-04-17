// #![allow(warnings)]
mod ident;
mod manifest;

use proc_macro2::TokenStream;
use quote::quote;
use std::path::{Path, PathBuf};

fn resolve_path(path: impl AsRef<Path>) -> PathBuf {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    if root.join(path.as_ref()).exists() {
        root.join(path.as_ref())
    } else {
        root.join("src/").join(&path.as_ref())
    }
}

fn quote_option<T: quote::ToTokens>(value: &Option<T>) -> TokenStream {
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
    let (struct_name, generics, manifest_path) = parse_derive(&ast);

    // match &ast.data {
    //     syn::Data::Struct(syn::DataStruct { fields, .. }) => {
    //         if !fields.is_empty() {
    //             panic!("Action can only be derived for empty structs")
    //         }
    //     },
    //     _ => panic!("Action can only be derived for structs"),
    // }

    let manifest = manifest::Manifest::from_action_yml(manifest_path);
    // dbg!(&manifest);

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let derived_methods: TokenStream = manifest
        .inputs
        .iter()
        .map(|(name, _input)| {
            let fn_name = ident::str_to_ident(&name);
            quote! {
                pub fn #fn_name<T>() -> Result<Option<T>, <T as ::actions::ParseInput>::Error>
                where T: ::actions::ParseInput {
                    ::actions::get_input::<T>(#name)
                }
            }
        })
        .collect();

    let input_enum_variants: Vec<_> = manifest
        .inputs
        .iter()
        .map(|(name, _)| {
            let variant = ident::str_to_enum_variant(&name);
            quote! { #variant }
        })
        .collect();

    let input_enum_matches: Vec<_> = manifest
        .inputs
        .iter()
        .map(|(name, _)| {
            let variant = ident::str_to_enum_variant(&name);
            quote! { #name => Ok(Self::#variant) }
        })
        .collect();

    let input_enum_ident = quote::format_ident!("{}Input", struct_name);
    let input_enum = quote! {
        #[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
        pub enum #input_enum_ident {
            #(#input_enum_variants,)*
        }

        impl std::str::FromStr for #input_enum_ident {
            type Err = ();
            fn from_str(input: &str) -> Result<Self, Self::Err> {
                match input {
                    #(#input_enum_matches,)*
                    _  => Err(()),
                }
            }
        }
    };
    // eprintln!("{}", pretty_print(&quote! { #input_enum }));

    let inputs: Vec<_> = manifest
        .inputs
        .iter()
        .map(|(name, input)| {
            let description = quote_option(&input.description);
            let deprecation_message = quote_option(&input.deprecation_message);
            let r#default = quote_option(&input.default);
            let required = quote_option(&input.required);
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
            type Input = #input_enum_ident;

            fn parse<E: ::actions::ReadEnv>(env: &E) -> std::collections::HashMap<Self::Input, Option<String>> {
                Self::inputs().iter().filter_map(|(name, input)| {
                    let value = ::actions::get_input_from::<String>(env, name);
                    let default = input.default.map(|s| s.to_string());
                    match std::str::FromStr::from_str(&name) {
                        Ok(variant) => Some((variant, value.unwrap().or(default))),
                        Err(_) => None,
                    }
                }).collect()
            }
        }
    };

    let manifest::Manifest {
        name,
        description,
        author,
        ..
    } = manifest;

    let input_impl = quote! {
        #[allow(clippy::all)]
        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Inputs of this action.
            pub fn inputs() -> ::std::collections::HashMap<
                &'static str, ::actions::Input<'static>
            > {
                static inputs: &'static [(&'static str, ::actions::Input<'static>)] = &[
                    #(#inputs,)*
                ];
                inputs.iter().cloned().collect()
            }

            /// Description of this action.
            pub fn description() -> &'static str {
                #description
            }

            /// Name of this action.
            pub fn name() -> &'static str {
                #name
            }

            /// Author of this trait.
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
    // eprintln!("{}", pretty_print(&tokens));
    tokens.into()
}

#[allow(dead_code)]
fn pretty_print(tokens: &TokenStream) -> String {
    let _file = syn::parse_file(&tokens.to_string()).unwrap();
    // TODO: this will not work until prettyplease updates to syn 2+
    // prettyplease::unparse(&file);
    tokens.to_string()
}
