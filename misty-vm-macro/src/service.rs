use proc_macro2::Ident;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2,
};

struct ServiceStruct {
    marker_token: Ident,
    _comma: syn::Token![,],
    impl_token: Ident,
}

impl Parse for ServiceStruct {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(ServiceStruct {
            marker_token: input.parse()?,
            _comma: input.parse()?,
            impl_token: input.parse()?,
        })
    }
}

pub fn parse_misty_service(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let input = parse2::<ServiceStruct>(input);
    if let Err(err) = input {
        panic!("parse misty service error: {}", err);
    }
    let input = input.unwrap();

    let marker_name = input.marker_token;
    let impl_name = input.impl_token;

    let output: proc_macro2::TokenStream = quote! {
        use misty_vm::services::*;
        use misty_vm::once_cell::sync::Lazy;
        use std::sync::Arc;

        pub struct #marker_name {
            ptr: Box<dyn #impl_name>,
        }
        const _: () = {
            impl MistyServiceTrait for #marker_name {

            }
            impl #marker_name {
                pub fn new(service: impl #impl_name + 'static) -> Self {
                    Self {
                        ptr: Box::new(service),
                    }
                }
            }
            impl std::ops::Deref for #marker_name {
                type Target = Box<dyn #impl_name>;

                fn deref(&self) -> &Self::Target {
                    &self.ptr
                }
            }
        };
    };
    output
}
