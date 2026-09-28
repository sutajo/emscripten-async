//! Implementation of `emscripten_futures::{test, bench}`.
#![doc = include_str!("../README.md")]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

#[derive(Clone, Copy)]
enum Harness {
    Test,
    Bench,
}

impl Harness {
    fn name(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Bench => "bench",
        }
    }
}

/// Wraps an async test in the Emscripten executor.
#[proc_macro_attribute]
pub fn test(args: TokenStream, input: TokenStream) -> TokenStream {
    attribute(args, input, Harness::Test)
}

/// Benchmarks a zero-argument async function, awaiting each measured iteration.
/// Requires nightly Rust, `#![feature(test)]`, and `extern crate test` in the caller.
#[proc_macro_attribute]
pub fn bench(args: TokenStream, input: TokenStream) -> TokenStream {
    attribute(args, input, Harness::Bench)
}

fn attribute(args: TokenStream, input: TokenStream, harness: Harness) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            Span::call_site(),
            format!("this {} attribute takes no arguments", harness.name()),
        )
        .into_compile_error()
        .into();
    }
    let function = parse_macro_input!(input as ItemFn);
    expand(function, harness)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(mut function: ItemFn, harness: Harness) -> syn::Result<proc_macro2::TokenStream> {
    validate(&function, harness)?;
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
    match harness {
        Harness::Test => {
            function.block = syn::parse_quote!({
                ::#runtime::executor::block_on(async #body)
            });
        }
        Harness::Bench => {
            let output = std::mem::replace(&mut function.sig.output, syn::ReturnType::Default);
            let bencher = syn::Ident::new("__emscripten_futures_bencher", Span::mixed_site());
            let iteration = syn::Ident::new("__emscripten_futures_iteration", Span::mixed_site());
            function.sig.inputs = syn::parse_quote!(#bencher: &mut ::test::Bencher);
            function.block = syn::parse_quote!({
                async fn #iteration() #output #body
                #bencher.iter(|| ::#runtime::executor::block_on(#iteration()));
            });
        }
    }
    let attribute = syn::Ident::new(harness.name(), Span::call_site());
    // Fully qualify the built-in attribute so importing our harness macro
    // cannot cause the generated attribute to recursively expand itself.
    Ok(quote! {
        #[::core::prelude::v1::#attribute]
        #function
    })
}

fn validate(function: &ItemFn, harness: Harness) -> syn::Result<()> {
    let signature = &function.sig;
    let name = harness.name();
    if signature.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            signature,
            format!("{name} function must be async"),
        ));
    }
    if !signature.inputs.is_empty() || signature.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            signature,
            format!("{name} function must take no arguments"),
        ));
    }
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            signature,
            format!("{name} function must not be generic"),
        ));
    }
    if signature.unsafety.is_some() || signature.constness.is_some() || signature.abi.is_some() {
        return Err(syn::Error::new_spanned(
            signature,
            format!("{name} function must be a safe Rust function"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Harness, validate};
    use syn::ItemFn;

    #[test]
    fn rejects_unsupported_signatures() {
        for (source, message) in [
            ("fn example() {}", "test function must be async"),
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
            for harness in [Harness::Test, Harness::Bench] {
                assert_eq!(
                    validate(&function, harness).unwrap_err().to_string(),
                    message.replace("test", harness.name()),
                );
            }
        }
    }

    #[test]
    fn validates_harness_arguments() {
        let function = syn::parse_str::<ItemFn>("async fn example() {}").unwrap();
        for harness in [Harness::Test, Harness::Bench] {
            validate(&function, harness).unwrap();
        }
        for source in [
            "async fn example(value: u32) {}",
            "async fn example(b: &mut test::Bencher) {}",
            "async fn example(b: &mut RenamedBencher) {}",
            "async fn example(b: &test::Bencher) {}",
            "async fn example(b: &mut test::Bencher, extra: u32) {}",
            "async fn example(&mut self) {}",
        ] {
            let function = syn::parse_str::<ItemFn>(source).unwrap();
            assert_eq!(
                validate(&function, Harness::Bench).unwrap_err().to_string(),
                "bench function must take no arguments"
            );
            assert_eq!(
                validate(&function, Harness::Test).unwrap_err().to_string(),
                "test function must take no arguments"
            );
        }
    }
}
