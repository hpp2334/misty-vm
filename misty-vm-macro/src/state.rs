use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2,
    punctuated::Punctuated,
};

#[derive(Debug)]

struct StatesStruct {
    states: Punctuated<syn::Type, syn::Token![,]>,
}

impl Parse for StatesStruct {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(StatesStruct {
            states: Punctuated::<syn::Type, syn::Token![,]>::parse_terminated(input)?,
        })
    }
}

pub fn parse_misty_states(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let input = parse2::<StatesStruct>(input);
    if input.is_err() {
        let err = input.unwrap_err();
        panic!("parse misty state error: {}", err);
    }

    let input = input.unwrap();
    let state_types: Vec<syn::Type> = input.states.clone().into_iter().collect();

    let output: proc_macro2::TokenStream = quote! {
        {
            use misty_vm::client::MistyClientId;
            use misty_vm::states::*;
            use misty_vm::once_cell::sync::Lazy;
            use std::collections::HashMap;
            use std::sync::RwLock;

            #(
                impl MistyStateTrait for #state_types {}
            )*

            let mut states = States::new();
            #(
                states.register::<#state_types>();
            )*
            states
        }
    };

    output
}
