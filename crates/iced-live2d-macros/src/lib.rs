//! The `Vocabulary` derive behind `iced-live2d`'s optional `macros` feature.
//!
//! ```text
//! #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
//! #[vocabulary(prefix = "Param")]
//! pub enum FaceParameter {
//!     MouthOpenY,
//!     EyeLOpen,
//!     #[name("ParamEyeBallX")]
//!     EyeBallX,
//! }
//! ```
//!
//! It also emits `const ALL: &'static [Self]`, so a host can enumerate its own
//! vocabulary without asking the model.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Error, Fields, LitStr, Result, parse_macro_input};

/// Derives `Vocabulary` for a fieldless enum.
///
/// `#[vocabulary(prefix = "Param")]` prefixes every variant name, `#[name("...")]`
/// overrides one. Without either, the variant name is the model name.
#[proc_macro_derive(Vocabulary, attributes(vocabulary, name))]
pub fn derive_vocabulary(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> Result<TokenStream2> {
    let name = &input.ident;

    let Data::Enum(data) = &input.data else {
        return Err(Error::new(
            input.span(),
            "Vocabulary can only be derived for an enum of unit variants",
        ));
    };

    let prefix = vocabulary_prefix(&input.attrs)?;

    let mut variants = Vec::with_capacity(data.variants.len());
    let mut declared: Vec<String> = Vec::with_capacity(data.variants.len());

    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(Error::new(
                variant.span(),
                "Vocabulary variants must be unit variants: a key is its name and its index",
            ));
        }

        let ident = &variant.ident;
        let text = match explicit_name(&variant.attrs)? {
            Some(text) => text,
            None => format!("{prefix}{ident}"),
        };

        if let Some(duplicate) = declared.iter().find(|declared| **declared == text) {
            return Err(Error::new(
                variant.span(),
                format!("`{duplicate}` is declared twice, so it has no single index"),
            ));
        }

        declared.push(text.clone());
        variants.push((ident.clone(), text));
    }

    let idents: Vec<_> = variants.iter().map(|(ident, _)| ident).collect();
    let names: Vec<_> = variants.iter().map(|(_, text)| text).collect();
    let indices: Vec<_> = (0..variants.len()).collect();

    Ok(quote! {
        impl ::iced_live2d::Vocabulary for #name {
            const SPEC: ::iced_live2d::VocabularySpec =
                ::iced_live2d::VocabularySpec::Static(&[#(#names),*]);

            fn name(&self) -> &str {
                match *self {
                    #(Self::#idents => #names,)*
                }
            }

            fn index(&self) -> usize {
                match *self {
                    #(Self::#idents => #indices,)*
                }
            }

            fn from_id(
                id: &::iced_live2d::KeyId,
            ) -> ::core::result::Result<Self, ::iced_live2d::VocabularyError> {
                Self::ALL
                    .get(id.index)
                    .cloned()
                    .ok_or(::iced_live2d::VocabularyError::new(id.index))
            }
        }

        impl #name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[#(Self::#idents),*];
        }
    })
}

fn vocabulary_prefix(attributes: &[Attribute]) -> Result<String> {
    let mut prefix = None;

    for attribute in attributes {
        if !attribute.path().is_ident("vocabulary") {
            continue;
        }

        attribute.parse_nested_meta(|meta| {
            if !meta.path.is_ident("prefix") {
                return Err(meta.error("the only `vocabulary` option is `prefix`"));
            }

            let value: LitStr = meta.value()?.parse()?;
            prefix = Some(value.value());

            Ok(())
        })?;
    }

    Ok(prefix.unwrap_or_default())
}

fn explicit_name(attributes: &[Attribute]) -> Result<Option<String>> {
    let mut name = None;

    for attribute in attributes {
        if !attribute.path().is_ident("name") {
            continue;
        }

        let value: LitStr = attribute.parse_args()?;
        name = Some(value.value());
    }

    Ok(name)
}
