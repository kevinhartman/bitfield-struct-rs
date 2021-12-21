//! # Bitfield Struct
//!
//! Procedural macro for bitfields that allows specifying bitfields as structs.
//! As this library provides a procedural-macro it has no runtime dependencies and works for `no-std`.
//!
//! ## Example
//!
//! ```ignore
//! #[bitfield(u64)]
//! struct PageTableEntry {
//!     /// defaults to 32 bits for u32
//!     addr: u32,
//!
//!     /// public field -> public accessor functions
//!     #[bits(12)]
//!     pub size: usize,
//!
//!     /// padding: No accessor functions are generated for fields beginning with `_`.
//!     #[bits(6)]
//!     _p: u8,
//!
//!     /// interpreted as 1 bit flag
//!     present: bool,
//!
//!     /// sign extend for signed integers
//!     #[bits(13)]
//!     negative: i16,
//! }
//! ```
//!
//! The macro generates three accessor functions for each field.
//! Each accessor also inherits the documentation of its field.
//!
//! The signatures for `addr` for example are:
//!
//! ```ignore
//! struct PageTableEntry(u64);
//! impl PageTableEntry {
//!     fn new() -> Self { /* ... */ }
//!
//!     fn with_addr(self, value: u32) -> Self { /* ... */ }
//!     fn addr(&self) -> u32 { /* ... */ }
//!     fn set_addr(&mut self, value: u32) { /* ... */ }
//!
//!     // other members ...
//! }
//! impl From<u64> for PageTableEntry { /* ... */ }
//! impl Into<u64> for PageTableEntry { /* ... */ }
//! ```
//!
//! This generated bitfield then can be used as follows.
//!
//! ```ignore
//! let pte = PageTableEntry::new()
//!     .with_addr(3 << 31)
//!     .with_size(2)
//!     .with_present(false)
//!     .with_negative(-3);
//!
//! println!("{}", pte.addr());
//!
//! pte.set_size(1);
//!
//! let value: u64 = pte.into();
//! println!("{:b}", value);
//! ```

use proc_macro as pc;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{AttrStyle, Attribute, LitInt, Type};

#[proc_macro_attribute]
pub fn bitfield(args: pc::TokenStream, input: pc::TokenStream) -> pc::TokenStream {
    match bitfield_inner(args.into(), input.into()) {
        Ok(result) => result.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

fn bitfield_inner(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let input = syn::parse2::<syn::ItemStruct>(input)?;
    let Params { ty, bits } = syn::parse2::<Params>(args)?;

    let span = input.fields.span();
    let name = input.ident;
    let vis = input.vis;

    let mut offset = 0;
    let mut members = TokenStream::new();
    match input.fields {
        syn::Fields::Named(fields) => {
            for field in fields.named {
                members.extend(bitfield_member(field, &ty, &mut offset)?);
            }
        }
        _ => return Err(syn::Error::new(span, "only named fields are supported")),
    };

    if offset != bits {
        return Err(syn::Error::new(
            span,
            format!(
                "The bitfiled size has to be equal to the sum of its members! {} != {}. \
                Padding can be done by prefixing members with \"_\". \
                For these members no accessor methods are generated.",
                bits, offset
            ),
        ));
    }

    Ok(quote! {
        #[derive(Copy, Clone)]
        #vis struct #name(#ty);

        impl #name {
            #vis fn new() -> Self {
                Self(0)
            }

            #members
        }

        impl From<#ty> for #name {
            fn from(v: #ty) -> Self {
                Self(v)
            }
        }
        impl Into<#ty> for #name {
            fn into(self) -> #ty {
                self.0
            }
        }
    })
}

fn bitfield_member(f: syn::Field, pty: &Type, offset: &mut usize) -> syn::Result<TokenStream> {
    let ty = &f.ty;

    let mut bits = type_bits(ty)?;
    match attr_bits(&f.attrs)? {
        Some(b) => {
            if b > bits {
                return Err(syn::Error::new_spanned(&f, "member type not large enough"));
            }
            if b == 0 {
                return Err(syn::Error::new_spanned(&f, "bits may not be 0"));
            }
            bits = b;
        }
        _ => {}
    }

    let doc: TokenStream = f
        .attrs
        .iter()
        .filter(|a| a.path.is_ident("doc"))
        .map(|a| a.to_token_stream())
        .collect();

    let start = *offset;
    *offset = start + bits;

    // Skip if unnamed
    let name = if let Some(name) = &f.ident {
        name
    } else {
        return Ok(TokenStream::new());
    };
    if name.to_string().starts_with('_') {
        return Ok(TokenStream::new());
    }

    let with_name = format_ident!("with_{}", name);
    let set_name = format_ident!("set_{}", name);
    let vis = &f.vis;

    if bits > 1 {
        Ok(quote! {
            #doc
            #vis fn #with_name(mut self, value: #ty) -> Self {
                self.#set_name(value);
                self
            }
            #doc
            #vis fn #name(&self) -> #ty {
                (((self.0 >> #start) as #ty) << #ty::BITS as usize - #bits) >> #ty::BITS as usize - #bits
            }
            #doc
            #vis fn #set_name(&mut self, value: #ty) {
                self.0 &= !(((1 << #bits) - 1) << #start);
                self.0 |= (value as #pty & ((1 << #bits) - 1)) << #start;
            }
        })
    } else {
        Ok(quote! {
            #doc
            #vis fn #with_name(mut self, value: #ty) -> Self {
                self.#set_name(value);
                self
            }
            #doc
            #vis fn #name(&self) -> #ty {
                ((self.0 >> #start) & 1) != 0
            }
            #doc
            #vis fn #set_name(&mut self, value: #ty) {
                self.0 &= !(1 << #start);
                self.0 |= (value as #pty & 1) << #start;
            }
        })
    }
}

fn attr_bits(attrs: &[Attribute]) -> syn::Result<Option<usize>> {
    fn malformed(mut e: syn::Error, attr: &Attribute) -> syn::Error {
        e.combine(syn::Error::new_spanned(attr, "malformed #[bits] attribute"));
        e
    }

    for attr in attrs {
        match attr {
            Attribute {
                pound_token: _,
                style: AttrStyle::Outer,
                bracket_token: _,
                path,
                tokens: _,
            } if path.is_ident("bits") => {
                return Ok(Some(
                    attr.parse_args::<LitInt>()
                        .map_err(|e| malformed(e, attr))?
                        .base10_parse()
                        .map_err(|e| malformed(e, attr))?,
                ))
            }
            _ => {}
        }
    }
    Ok(None)
}

fn type_bits(ty: &Type) -> syn::Result<usize> {
    match ty {
        Type::Path(path) if path.path.is_ident("bool") => Ok(1),
        Type::Path(path) if path.path.is_ident("u8") => Ok(u8::BITS as _),
        Type::Path(path) if path.path.is_ident("i8") => Ok(i8::BITS as _),
        Type::Path(path) if path.path.is_ident("u16") => Ok(u16::BITS as _),
        Type::Path(path) if path.path.is_ident("i16") => Ok(i16::BITS as _),
        Type::Path(path) if path.path.is_ident("u32") => Ok(u32::BITS as _),
        Type::Path(path) if path.path.is_ident("i32") => Ok(i32::BITS as _),
        Type::Path(path) if path.path.is_ident("u64") => Ok(u64::BITS as _),
        Type::Path(path) if path.path.is_ident("i64") => Ok(i64::BITS as _),
        Type::Path(path) if path.path.is_ident("u128") => Ok(u128::BITS as _),
        Type::Path(path) if path.path.is_ident("i128") => Ok(i128::BITS as _),
        Type::Path(path) if path.path.is_ident("usize") => Ok(usize::BITS as _),
        Type::Path(path) if path.path.is_ident("isize") => Ok(isize::BITS as _),
        _ => Err(syn::Error::new_spanned(ty, "unsupported type")),
    }
}

struct Params {
    ty: Type,
    bits: usize,
}

impl Parse for Params {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if let Ok(ty) = Type::parse(input) {
            Ok(Params {
                bits: type_bits(&ty).map_err(|mut e| {
                    e.combine(unsupported_arg(input.span()));
                    e
                })?,
                ty,
            })
        } else {
            Err(unsupported_arg(input.span()))
        }
    }
}

fn unsupported_arg<T>(arg: T) -> syn::Error
where
    T: syn::spanned::Spanned,
{
    syn::Error::new(arg.span(), "unsupported #[bitfield] argument")
}
