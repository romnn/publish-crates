#![allow(warnings)]
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use std::collections::HashMap;
use std::path::PathBuf;

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
    default: Option<String>,
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

    inputs: HashMap<String, Input>,
}

#[proc_macro_derive(Action)]
pub fn action_derive(input: TokenStream) -> TokenStream {
    let tokens: TokenStream = quote! {}.into();
    // eprintln!("TOKENS: {}", tokens);
    tokens
}

#[proc_macro_attribute]
pub fn action(attrs: TokenStream, input: TokenStream) -> TokenStream {
    eprintln!("{:#?}", attrs);
    let attrs = syn::parse_macro_input!(attrs as syn::AttributeArgs);
    let ast = syn::parse_macro_input!(input as syn::DeriveInput);

    for attr in attrs {
        match attr {
            syn::NestedMeta::Lit(lit) => {
                match lit {
                    syn::Lit::Str(s) => {
                        let path = s.value();
                        let root = PathBuf::from(
                            std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()),
                        );
                        let path = if root.join(&path).exists() {
                            root.join(&path)
                        } else {
                            root.join("src/").join(&path)
                        };

                        eprintln!("manifest: {:#?}", path);
                        let mut manifest_file = std::io::BufReader::new(
                            std::fs::OpenOptions::new().read(true).open(path).unwrap(),
                        );
                        let manifest: Manifest =
                            serde_yaml::from_reader(manifest_file).unwrap();
                        eprintln!("manifest: {:#?}", manifest);
                    }
                    _ => {}
                }
            }
            syn::NestedMeta::Meta(syn::Meta::Path(p)) => {
                eprintln!("path: {:#?}", p.to_token_stream());
            }
            _ => {}
        }
    }

    let name = ast.ident;
    eprintln!("{}", name);

    let fields = match ast.data {
        syn::Data::Struct(syn::DataStruct {
            fields: syn::Fields::Named(syn::FieldsNamed { ref named, .. }),
            ..
        }) => named,
        _ => unimplemented!("action macro can only be used on structs"),
    };

    let builder_fields = fields.iter().map(|f| {
        let name = &f.ident;
        let ty = &f.ty;
        for attr in &f.attrs {
            // if attr.path().is_ident("repr") {}
        }
        eprintln!("field {:?}: {:#?}", name, ty.to_token_stream());

        quote! { pub #name: #ty } // set the field to public
    });

    let tokens = quote! {
        pub struct #name {
            #(#builder_fields,)*
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
