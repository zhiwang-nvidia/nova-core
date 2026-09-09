// SPDX-License-Identifier: GPL-2.0

use proc_macro2::TokenStream;
use quote::{
    format_ident,
    quote, //
};
use syn::{
    parse::{
        Parse,
        ParseStream, //
    },
    Attribute,
    Error,
    FnArg,
    Ident,
    ImplItem,
    ItemImpl,
    Path,
    Result,
    ReturnType,
    Token, //
};

pub(crate) struct FfiVtableArgs {
    table: Ident,
    ops: Path,
}

impl Parse for FfiVtableArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let table = input.parse()?;
        let _: Token![:] = input.parse()?;
        let ops = input.parse()?;

        Ok(Self { table, ops })
    }
}

fn has_conditional(attributes: &[Attribute]) -> bool {
    attributes
        .iter()
        .any(|attribute| attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr"))
}

pub(crate) fn ffi_vtable(args: FfiVtableArgs, item: ItemImpl) -> Result<TokenStream> {
    if item.trait_.is_some()
        || !item.generics.params.is_empty()
        || item.generics.where_clause.is_some()
    {
        return Err(Error::new_spanned(
            &item,
            "`#[ffi_vtable]` requires a concrete, non-generic inherent impl",
        ));
    }
    if has_conditional(&item.attrs) {
        return Err(Error::new_spanned(
            &item,
            "`#[ffi_vtable]` does not support conditionally compiled impls",
        ));
    }

    let ops = &args.ops;
    let table = &args.table;
    let self_ty = &item.self_ty;
    let private = quote!(::kernel::interop::ffi::__private);
    let mut fields = Vec::new();

    for impl_item in &item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let signature = &method.sig;
        if has_conditional(&method.attrs)
            || !signature.generics.params.is_empty()
            || signature.generics.where_clause.is_some()
        {
            return Err(Error::new_spanned(
                method,
                "`#[ffi_vtable]` requires unconditional, non-generic methods",
            ));
        }

        let mut argument_names = Vec::new();
        let mut argument_types = Vec::new();
        for argument in &signature.inputs {
            let FnArg::Typed(argument) = argument else {
                continue;
            };

            let index = argument_names.len();
            argument_names.push(format_ident!("__ffi_vtable_arg_{index}"));
            argument_types.push(&argument.ty);
        }

        let method_name = &signature.ident;
        let rust_output = match &signature.output {
            ReturnType::Default => quote!(()),
            ReturnType::Type(_, ty) => quote!(#ty),
        };

        fields.push(quote! {
            #method_name: ::core::option::Option::Some({
                unsafe extern "C" fn callback<__FfiVtableReturn>(
                    __ffi_vtable_context: *const ::core::ffi::c_void,
                    #(#argument_names: #argument_types),*
                ) -> __FfiVtableReturn
                where
                    #rust_output: #private::FfiReturn<__FfiVtableReturn>,
                {
                    // The inferred receiver type ties any inner lifetime in `Self` to the context.
                    let __ffi_vtable_method: unsafe fn(
                        ::core::pin::Pin<&_>,
                        #(#argument_types),*
                    ) -> #rust_output = <#self_ty>::#method_name;

                    // SAFETY: The publisher keeps the context live and pinned while callbacks run.
                    let __ffi_vtable_this = unsafe {
                        #private::borrow_context::<#self_ty>(__ffi_vtable_context)
                    };

                    #private::FfiReturn::<__FfiVtableReturn>::into_ffi(
                        // SAFETY: An unsafe method relies on the C caller satisfying its argument
                        // contract.
                        unsafe {
                            __ffi_vtable_method(__ffi_vtable_this, #(#argument_names),*)
                        },
                    )
                }

                callback::<_>
            })
        });
    }

    Ok(quote! {
        #item

        static #table: #ops = #ops {
            #(#fields),*
        };
    })
}
