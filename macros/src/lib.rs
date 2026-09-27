//! Implementation of `emscripten_futures::test`.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// Wraps an async test in the Emscripten executor.
#[proc_macro_attribute]
pub fn test(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(Span::call_site(), "this test attribute takes no arguments")
            .into_compile_error()
            .into();
    }
    let function = parse_macro_input!(input as ItemFn);
    expand(function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(mut function: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    validate(&function)?;
    let name = match crate_name("emscripten-futures") {
        // The library aliases itself under this name, so this also works in
        // its unit tests, integration tests, and documentation examples.
        Ok(FoundCrate::Itself) => "emscripten_futures".to_owned(),
        Ok(FoundCrate::Name(name)) => name,
        Err(error) => return Err(syn::Error::new(Span::call_site(), error)),
    };
    let runtime = syn::Ident::new(&name, Span::call_site());
    function.sig.asyncness = None;
    let body = function.block;
    function.block = syn::parse_quote!({
        ::#runtime::executor::block_on(async #body)
    });
    // Fully qualify the built-in attribute so importing our `test` macro
    // cannot cause the generated attribute to recursively expand itself.
    Ok(quote! {
        #[::core::prelude::v1::test]
        #function
    })
}

fn validate(function: &ItemFn) -> syn::Result<()> {
    let signature = &function.sig;
    if signature.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            signature,
            "test function must be async",
        ));
    }
    if !signature.inputs.is_empty() || signature.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            signature,
            "test function must take no arguments",
        ));
    }
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            signature,
            "test function must not be generic",
        ));
    }
    if signature.unsafety.is_some() || signature.constness.is_some() || signature.abi.is_some() {
        return Err(syn::Error::new_spanned(
            signature,
            "test function must be a safe Rust function",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;
    use syn::ItemFn;

    #[test]
    fn rejects_unsupported_signatures() {
        for (source, message) in [
            ("fn example() {}", "test function must be async"),
            (
                "async fn example(value: u32) {}",
                "test function must take no arguments",
            ),
            (
                "async fn example<T>() {}",
                "test function must not be generic",
            ),
            (
                "async fn example() where (): Sized {}",
                "test function must not be generic",
            ),
            (
                "async unsafe fn example() {}",
                "test function must be a safe Rust function",
            ),
            (
                "async extern \"C\" fn example() {}",
                "test function must be a safe Rust function",
            ),
        ] {
            let function = syn::parse_str::<ItemFn>(source).unwrap();
            assert_eq!(validate(&function).unwrap_err().to_string(), message);
        }
    }
}
