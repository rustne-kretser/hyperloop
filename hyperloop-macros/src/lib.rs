#![feature(proc_macro_diagnostic)]
#![feature(box_into_inner)]

use darling::FromMeta;
use proc_macro::{self, TokenStream};
use syn::{FnArg, Ident, Pat, punctuated::{Pair, Punctuated}, spanned::Spanned, token::Comma};
use quote::{format_ident, quote};

#[derive(Debug, FromMeta)]
struct TaskArgs {
    priority: u8,
}

// This macro has liberally borrowed from embassy
#[proc_macro_attribute]
pub fn task(args: TokenStream, item: TokenStream) -> TokenStream {
    let macro_args = syn::parse_macro_input!(args as syn::AttributeArgs);
    let mut task_fn = syn::parse_macro_input!(item as syn::ItemFn);

    let macro_args = match TaskArgs::from_list(&macro_args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors());
        }
    };

    let priority = macro_args.priority;

    if task_fn.sig.asyncness.is_none() {
        task_fn
            .sig
            .span()
            .unwrap()
            .error("task functions must be async")
            .emit();
        return TokenStream::new();
    }

    let name = task_fn.sig.ident.clone();
    let args = task_fn.sig.inputs.clone();
    let arg_values: Punctuated<Ident, Comma> = args
            .pairs()
            .filter_map(|pair| {
                let (arg, punct) = pair.into_tuple();

                if let FnArg::Typed(pat_type) = arg {
                    if let Pat::Ident(pat_ident) = *pat_type.pat.clone() {
                        Some(Pair::new(pat_ident.ident, punct.copied()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

    let visibility = &task_fn.vis;
    task_fn.sig.ident = format_ident!("task");
    let future_type = quote!(impl ::core::future::Future<Output = ()> + 'static);
    let attrs = &task_fn.attrs;

    let result = quote! {
        #(#attrs)*
        #visibility fn #name(#args) -> Option<&'static mut crate::task::Task<#future_type>> {
            type F = #future_type;

            fn wrapper(#args) -> impl FnOnce() -> F {
                move || {
                    #task_fn
                    task(#arg_values)
                }
            }

            static mut TASK: Option<Task<F>> = None;

            unsafe {
                if let None = TASK {
                    TASK = Some(Task::new(wrapper(#arg_values), #priority));
                    Some(TASK.as_mut().unwrap())
                } else {
                    None
                }
            }
        }
    };
    result.into()
}
