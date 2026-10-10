//! Derive macros for [`cubic-core`]'s reflection layer.
//!
//! Two derives, deliberately separate (the plan splits them the same way):
//!
//! - `#[derive(Component)]` gives a type its stable identity — the name a
//!   `.rsn` file and the inspector refer to it by, plus its [`TypeId`].
//! - `#[derive(Inspectable)]` describes the type's editable fields, one
//!   [`Field`](::cubic_core::reflect::Field) per public-by-default field,
//!   each carrying the kind the inspector draws it with.
//!
//! A component usually derives both: `#[derive(Component, Inspectable)]`.
//!
//! Field kinds are inferred from the field's type (`f32`, `i32`, `bool`,
//! `Vec2`, `Rgba`, `String`, and the three asset handle types). A field of
//! any other type is skipped unless it is an enum-like one annotated with
//! `#[inspect(select = ["a", "b"])]`, which makes it a
//! [`Select`](::cubic_core::reflect::FieldKind::Select) of those labels. Use
//! `#[inspect(skip)]` to leave a field out entirely.
//!
//! [`cubic-core`]: https://docs.rs/cubic-core

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Expr, Lit, Type, parse_macro_input};

/// The path generated code reaches `cubic-core` through, as tokens.
///
/// The dependant crate must name its `cubic-core` dependency `cubic_core`,
/// which is the only spelling cargo allows a dependency to have anyway.
///
/// It is built here rather than used as a literal string because `quote!`
/// interpolates a `&str` as a string literal — the string form would come out
/// as `"::cubic_core::reflect"::Field`.
fn reflect() -> proc_macro2::TokenStream {
    syn::parse_str("::cubic_core::reflect").expect("the reflection path is a well-formed path")
}

/// `#[derive(Component)]`: the type's stable name and `TypeId`.
///
/// This is the identity half of reflection — what a scene file and the
/// inspector call the component — and carries no field information. Pair it
/// with [`Inspectable`](derive@Inspectable) to make the type editable.
#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    if let Err(error) = check_generics(&input) {
        return error.into_compile_error().into();
    }
    let reflect = reflect();
    let ident = &input.ident;
    let name = ident.to_string();
    quote! {
        #[automatically_derived]
        impl #reflect::ComponentInfo for #ident {
            fn name() -> &'static str {
                #name
            }

            fn type_id() -> ::std::any::TypeId {
                ::std::any::TypeId::of::<#ident>()
            }
        }
    }
    .into()
}

/// `#[derive(Inspectable)]`: one field descriptor per editable field.
///
/// The descriptors are what the inspector renders a property grid from, and
/// what prefabs and the scene layer read a component's shape through. Every
/// included field becomes a [`Field`](::cubic_core::reflect::Field) whose
/// `get`/`set` read and write it through type-erased `Any`, so nothing about
/// the concrete type escapes the descriptor.
///
/// Recognised field attributes (the `inspect` helper attribute is declared by
/// this derive):
///
/// - `#[inspect(skip)]` — leave the field out of the descriptors.
/// - `#[inspect(select = ["idle", "walk", "run"])]` — treat the field as a
///   variant index (its type must implement
///   [`Selectable`](::cubic_core::reflect::Selectable)) over exactly these
///   labels, in this order.
#[proc_macro_derive(Inspectable, attributes(inspect))]
pub fn derive_inspectable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    if let Err(error) = check_generics(&input) {
        return error.into_compile_error().into();
    }
    let reflect = reflect();
    let ident = &input.ident;
    let Data::Struct(data) = &input.data else {
        return syn::Error::new_spanned(ident, "Inspectable can only be derived for a struct")
            .into_compile_error()
            .into();
    };
    let syn::Fields::Named(fields) = &data.fields else {
        return syn::Error::new_spanned(
            ident,
            "Inspectable needs named fields: tuple and unit structs have nothing to name",
        )
        .into_compile_error()
        .into();
    };

    let mut descriptors = Vec::new();
    for field in &fields.named {
        let meta = match inspect_meta(field) {
            Ok(meta) => meta,
            Err(error) => return error.into_compile_error().into(),
        };
        if meta.skip {
            continue;
        }
        let Some(field_ident) = &field.ident else {
            continue;
        };
        let name = field_ident.to_string();
        let kind = match meta.select {
            Some(options) => FieldPlan::Select(options),
            None => match infer_kind(&field.ty) {
                Some(kind) => kind,
                None => continue,
            },
        };
        descriptors.push(describe(ident, field_ident, &field.ty, &name, kind));
    }

    quote! {
        #[automatically_derived]
        impl #reflect::Inspectable for #ident {
            fn fields() -> &'static [#reflect::Field] {
                static FIELDS: ::std::sync::OnceLock<&'static [#reflect::Field]> =
                    ::std::sync::OnceLock::new();
                FIELDS.get_or_init(|| {
                    ::std::boxed::Box::leak(::std::vec![#(#descriptors),*].into_boxed_slice())
                })
            }
        }
    }
    .into()
}

/// What an `inspect` attribute asked for.
#[derive(Default)]
struct InspectMeta {
    skip: bool,
    /// The option labels for a `select` field, in display order.
    select: Option<Vec<String>>,
}

/// The kind a field's type implies, when it implies one at all.
enum FieldPlan {
    F32,
    I32,
    Bool,
    Vec2,
    Color,
    /// A raw [`AssetHandle`](::cubic_core::assets::AssetHandle).
    Asset,
    /// A [`TextureHandle`](::cubic_core::assets::TextureHandle).
    Texture,
    /// A [`FontHandle`](::cubic_core::assets::FontHandle).
    Font,
    String,
    Select(Vec<String>),
}

/// One `#[inspect(...)]` attribute's worth of metadata.
fn inspect_meta(field: &syn::Field) -> syn::Result<InspectMeta> {
    let mut meta = InspectMeta::default();
    for attribute in &field.attrs {
        if !attribute.path().is_ident("inspect") {
            continue;
        }
        attribute.parse_nested_meta(|nested| {
            if nested.path.is_ident("skip") {
                meta.skip = true;
                return Ok(());
            }
            if nested.path.is_ident("select") {
                let value: Expr = nested.value()?.parse()?;
                let Expr::Array(array) = value else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "inspect(select) expects a list of option names: select = [\"a\", \"b\"]",
                    ));
                };
                let mut options = Vec::new();
                for element in &array.elems {
                    let Expr::Lit(expr) = element else {
                        return Err(syn::Error::new_spanned(
                            element,
                            "option names must be string literals",
                        ));
                    };
                    let Lit::Str(label) = &expr.lit else {
                        return Err(syn::Error::new_spanned(
                            element,
                            "option names must be string literals",
                        ));
                    };
                    options.push(label.value());
                }
                if options.is_empty() {
                    return Err(syn::Error::new_spanned(
                        array,
                        "inspect(select) needs at least one option name",
                    ));
                }
                meta.select = Some(options);
                return Ok(());
            }
            Err(nested.error("expected `skip` or `select = [...]`"))
        })?;
    }
    Ok(meta)
}

/// The descriptor `field` contributes, as the `Field::new` call for the
/// component's `fields()` slice.
///
/// The `get`/`set` closures downcast the type-erased component back to the
/// concrete struct and read or write the one field; a downcast that misses
/// means the registry routed the wrong type, which is a bug rather than a
/// condition, so it panics with the field named for the diagnosis.
fn describe(
    component: &syn::Ident,
    field: &syn::Ident,
    field_type: &Type,
    name: &str,
    plan: FieldPlan,
) -> proc_macro2::TokenStream {
    let reflect = reflect();
    let miss = syn::LitStr::new(
        &format!("reflection field `{name}` used on the wrong component type"),
        proc_macro2::Span::call_site(),
    );
    let name = syn::LitStr::new(name, proc_macro2::Span::call_site());

    let (kind, get, set) = match plan {
        FieldPlan::F32 => (
            quote!(#reflect::FieldKind::F32),
            quote!(#reflect::FieldValue::F32(value.#field)),
            quote!(value.#field = typed.f32()?;),
        ),
        FieldPlan::I32 => (
            quote!(#reflect::FieldKind::I32),
            quote!(#reflect::FieldValue::I32(value.#field)),
            quote!(value.#field = typed.i32()?;),
        ),
        FieldPlan::Bool => (
            quote!(#reflect::FieldKind::Bool),
            quote!(#reflect::FieldValue::Bool(value.#field)),
            quote!(value.#field = typed.boolean()?;),
        ),
        FieldPlan::Vec2 => (
            quote!(#reflect::FieldKind::Vec2),
            quote!(#reflect::FieldValue::Vec2(value.#field)),
            quote!(value.#field = typed.vec2()?;),
        ),
        FieldPlan::Color => (
            quote!(#reflect::FieldKind::Color),
            quote!(#reflect::FieldValue::Color(value.#field)),
            quote!(value.#field = typed.color()?;),
        ),
        FieldPlan::Asset => (
            quote!(#reflect::FieldKind::Asset {
                kind: ::cubic_core::assets::AssetKind::Bytes
            }),
            quote!(#reflect::FieldValue::Asset(value.#field)),
            quote!(value.#field = typed.asset()?;),
        ),
        // A typed handle stores the face; the descriptor speaks in the raw
        // handle it names, so a picker can take it without unwrapping and the
        // component gets the face back from the id it held.
        FieldPlan::Texture => (
            quote!(#reflect::FieldKind::Asset {
                kind: ::cubic_core::assets::AssetKind::Texture
            }),
            quote!(#reflect::FieldValue::Asset(value.#field.asset())),
            quote!(value.#field = typed.asset()?.into();),
        ),
        FieldPlan::Font => (
            quote!(#reflect::FieldKind::Asset {
                kind: ::cubic_core::assets::AssetKind::Font
            }),
            quote!(#reflect::FieldValue::Asset(value.#field.asset())),
            quote!(value.#field = typed.asset()?.into();),
        ),
        FieldPlan::String => (
            quote!(#reflect::FieldKind::String),
            quote!(#reflect::FieldValue::String(value.#field.clone())),
            quote!(value.#field = typed.string()?;),
        ),
        FieldPlan::Select(options) => {
            let options = options
                .iter()
                .map(|label| syn::LitStr::new(label, proc_macro2::Span::call_site()))
                .collect::<Vec<_>>();
            // Plain string literals, not `&"label"` references: an array of
            // references is not rvalue-promoted to `'static`, where an array
            // of literals is.
            let options = quote!(&[#(#options),*]);
            (
                quote!(#reflect::FieldKind::Select(#options)),
                quote!(#reflect::FieldValue::Select(
                    <#field_type as #reflect::Selectable>::index(value.#field)
                )),
                quote! {
                    let index = typed.select()?;
                    if index >= (#options).len() {
                        return Err(#reflect::FieldError::SelectOutOfRange(index));
                    }
                    value.#field = <#field_type as #reflect::Selectable>::from_index(index);
                },
            )
        }
    };

    quote! {
        #reflect::Field::new(
            #name,
            #kind,
            |any| {
                let value = any.downcast_ref::<#component>().expect(#miss);
                #get
            },
            |any, typed| {
                let value = any.downcast_mut::<#component>().expect(#miss);
                #set
                ::core::result::Result::Ok(())
            },
        )
    }
}

/// The `FieldKind` a field's type name implies, if it is one reflection knows.
///
/// Detection is by the last path segment, so `Vec2`, `glam::Vec2` and
/// `cubic_core::prelude::Vec2` all read as vectors — and so a type that
/// merely shares a name with one of these would read the same way. The
/// override is to skip the field or, for a `select`, annotate it.
fn infer_kind(field_type: &Type) -> Option<FieldPlan> {
    let Type::Path(path) = field_type else {
        return None;
    };
    let name = path.path.segments.last()?.ident.to_string();
    let plan = match name.as_str() {
        "f32" => FieldPlan::F32,
        "i32" => FieldPlan::I32,
        "bool" => FieldPlan::Bool,
        "Vec2" => FieldPlan::Vec2,
        "Rgba" => FieldPlan::Color,
        "String" => FieldPlan::String,
        "TextureHandle" => FieldPlan::Texture,
        "FontHandle" => FieldPlan::Font,
        "AssetHandle" => FieldPlan::Asset,
        _ => return None,
    };
    Some(plan)
}

/// Reflection is built for concrete component types; generics would need the
/// field closures to be generic too, which `Any` downcasting cannot express.
fn check_generics(input: &DeriveInput) -> syn::Result<()> {
    if input.generics.params.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new_spanned(
        &input.generics,
        "cubic reflection derives do not support generic components",
    ))
}
