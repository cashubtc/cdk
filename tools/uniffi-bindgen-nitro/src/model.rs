//! A normalised view of a `ComponentInterface`, shared by every emitter.

use std::collections::BTreeMap;

use uniffi_bindgen::interface::{AsType, Enum, Object, Record, Type};
use uniffi_bindgen::ComponentInterface;

use crate::config::NitroConfig;
use crate::error::{Error, Result};
use crate::naming::{lower_camel, pascal};
use crate::typemap::{map, Mapped};

/// One argument of an exported callable.
#[derive(Debug, Clone)]
pub struct Arg {
    /// Name as written in Rust.
    pub rust_name: String,
    /// Name used in TypeScript and in generated C++.
    pub js_name: String,
    /// The UniFFI type.
    pub ty: Type,
    /// Its spellings in each target language.
    pub mapped: Mapped,
}

/// What a callable is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallableKind {
    /// A free function, exposed as a method on the root hybrid object.
    Function,
    /// A constructor, exposed as a factory method on the root hybrid object.
    Constructor {
        /// Object being constructed.
        object: String,
    },
    /// An instance method on an object.
    Method {
        /// Object the method belongs to.
        object: String,
    },
}

/// An exported function, constructor or method.
#[derive(Debug, Clone)]
pub struct CallableModel {
    /// Name as written in Rust.
    pub rust_name: String,
    /// Method name in TypeScript.
    pub js_name: String,
    /// Function name in the generated bridge namespace.
    pub bridge_name: String,
    /// The `uniffi_*` symbol to call.
    pub symbol: String,
    /// Arguments, excluding the receiver.
    pub args: Vec<Arg>,
    /// Return type, if any.
    pub returns: Option<(Type, Mapped)>,
    /// Error enum this callable can raise.
    pub throws: Option<String>,
    /// Whether the FFI symbol takes a `RustCallStatus` out parameter.
    pub has_status: bool,
    /// What the callable hangs off.
    pub kind: CallableKind,
    /// Rust documentation, if any.
    pub docs: Option<String>,
}

/// A field of a record or an error variant.
#[derive(Debug, Clone)]
pub struct FieldModel {
    /// Name as written in Rust.
    pub rust_name: String,
    /// Name in TypeScript and generated C++.
    pub js_name: String,
    /// The UniFFI type.
    pub ty: Type,
    /// Its spellings in each target language.
    pub mapped: Mapped,
}

/// A `uniffi::Record`.
#[derive(Debug, Clone)]
pub struct RecordModel {
    /// Type name.
    pub name: String,
    /// Fields in declaration order, which is also the wire order.
    pub fields: Vec<FieldModel>,
    /// Rust documentation, if any.
    pub docs: Option<String>,
}

/// One variant of an enum or error enum.
#[derive(Debug, Clone)]
pub struct VariantModel {
    /// Variant name as written in Rust.
    pub rust_name: String,
    /// String literal used on the TypeScript side.
    pub js_name: String,
    /// Fields carried by the variant, in wire order.
    pub fields: Vec<FieldModel>,
}

/// A `uniffi::Enum` or `uniffi::Error`.
#[derive(Debug, Clone)]
pub struct EnumModel {
    /// Type name.
    pub name: String,
    /// Variants in declaration order; the wire tag is the 1-based index.
    pub variants: Vec<VariantModel>,
    /// Rust documentation, if any.
    pub docs: Option<String>,
}

/// A `uniffi::Object`.
#[derive(Debug, Clone)]
pub struct ObjectModel {
    /// Type name.
    pub name: String,
    /// The `uniffi_*` symbol that drops one reference.
    pub free_symbol: String,
    /// The `uniffi_*` symbol that clones a handle.
    pub clone_symbol: String,
    /// Instance methods.
    pub methods: Vec<CallableModel>,
    /// Rust documentation, if any.
    pub docs: Option<String>,
}

/// Everything the emitters need, derived once from the `ComponentInterface`.
#[derive(Debug)]
pub struct Model {
    /// Nitro module name, e.g. `CashuCrypto`.
    pub module: String,
    /// C++ namespace nested under `margelo::nitro`.
    pub cxx_namespace: String,
    /// Name of the cdylib the symbols live in.
    pub cdylib: String,
    /// Records, sorted by name.
    pub records: Vec<RecordModel>,
    /// Plain enums, sorted by name.
    pub enums: Vec<EnumModel>,
    /// Error enums, sorted by name.
    pub errors: Vec<EnumModel>,
    /// Objects, sorted by name.
    pub objects: Vec<ObjectModel>,
    /// Free functions plus every object constructor.
    pub root_methods: Vec<CallableModel>,
    /// Every type that needs a generated serializer, keyed by serializer name.
    pub serialized: BTreeMap<String, (Type, Mapped)>,
    /// Symbol that allocates a `RustBuffer`.
    pub rustbuffer_alloc: String,
    /// Symbol that frees a `RustBuffer`.
    pub rustbuffer_free: String,
    /// Symbol that copies foreign bytes into a `RustBuffer`.
    pub rustbuffer_from_bytes: String,
    /// Symbol returning the ABI contract version.
    pub contract_version_symbol: String,
    /// The ABI contract version the library was built against.
    pub contract_version: u32,
    /// Checksum symbols and their expected values, used as a build-time guard.
    pub checksums: Vec<(String, u16)>,
}

impl Model {
    /// Derive the model, rejecting anything Nitro cannot represent.
    pub fn build(ci: &ComponentInterface, config: &NitroConfig) -> Result<Self> {
        let mut model = Self {
            module: config.module_name.clone(),
            cxx_namespace: config.cxx_namespace.clone(),
            cdylib: config.cdylib_name.clone(),
            records: Vec::new(),
            enums: Vec::new(),
            errors: Vec::new(),
            objects: Vec::new(),
            root_methods: Vec::new(),
            serialized: BTreeMap::new(),
            rustbuffer_alloc: ci.ffi_rustbuffer_alloc().name().to_string(),
            rustbuffer_free: ci.ffi_rustbuffer_free().name().to_string(),
            rustbuffer_from_bytes: ci.ffi_rustbuffer_from_bytes().name().to_string(),
            contract_version_symbol: ci.ffi_uniffi_contract_version().name().to_string(),
            contract_version: ci.uniffi_contract_version(),
            checksums: ci.iter_checksums().collect(),
        };

        if ci.has_callback_definitions() {
            return Err(Error::UnsupportedFeature {
                context: ci.namespace().to_string(),
                feature: "callback interfaces".to_string(),
            });
        }

        for record in ci.record_definitions() {
            model.records.push(model_record(record)?);
        }
        for enum_ in ci.enum_definitions() {
            let is_error = ci.is_name_used_as_error(enum_.name());
            let modelled = model_enum(enum_, is_error)?;
            if is_error {
                model.errors.push(modelled);
            } else {
                model.enums.push(modelled);
            }
        }
        for object in ci.object_definitions() {
            model.push_object(object)?;
        }
        for function in ci.function_definitions() {
            let callable = model_callable(
                function.name(),
                lower_camel(function.name()),
                pascal(function.name()),
                function.ffi_func().name(),
                function.arguments().into_iter().cloned().collect(),
                function.return_type().cloned(),
                function.throws_name().map(pascal),
                function.ffi_func().has_rust_call_status_arg(),
                function.is_async(),
                CallableKind::Function,
                function.docstring().map(str::to_string),
            )?;
            model.root_methods.push(callable);
        }

        model.records.sort_by(|a, b| a.name.cmp(&b.name));
        model.enums.sort_by(|a, b| a.name.cmp(&b.name));
        model.errors.sort_by(|a, b| a.name.cmp(&b.name));
        model.root_methods.sort_by(|a, b| a.js_name.cmp(&b.js_name));
        model.collect_serialized()?;
        Ok(model)
    }

    fn push_object(&mut self, object: &Object) -> Result<()> {
        if object.is_trait_interface() || object.has_callback_interface() {
            return Err(Error::UnsupportedFeature {
                context: object.name().to_string(),
                feature: "trait interfaces implemented in the foreign language".to_string(),
            });
        }

        let name = pascal(object.name());
        let mut methods = Vec::new();
        for method in object.methods() {
            methods.push(model_callable(
                method.name(),
                lower_camel(method.name()),
                format!("{name}{}", pascal(method.name())),
                method.ffi_func().name(),
                method.arguments().into_iter().cloned().collect(),
                method.return_type().cloned(),
                method.throws_name().map(pascal),
                method.ffi_func().has_rust_call_status_arg(),
                method.is_async(),
                CallableKind::Method {
                    object: name.clone(),
                },
                method.docstring().map(str::to_string),
            )?);
        }
        methods.sort_by(|a, b| a.js_name.cmp(&b.js_name));

        for constructor in object.constructors() {
            let js_name = if constructor.is_primary_constructor() {
                format!("create{name}")
            } else {
                format!("create{name}{}", pascal(constructor.name()))
            };
            self.root_methods.push(model_callable(
                constructor.name(),
                lower_camel(&js_name),
                format!("{name}{}", pascal(constructor.name())),
                constructor.ffi_func().name(),
                constructor.arguments().into_iter().cloned().collect(),
                Some(Type::Object {
                    name: object.name().to_string(),
                    module_path: String::new(),
                    imp: object.imp().to_owned(),
                }),
                constructor.throws_name().map(pascal),
                constructor.ffi_func().has_rust_call_status_arg(),
                constructor.is_async(),
                CallableKind::Constructor {
                    object: name.clone(),
                },
                constructor.docstring().map(str::to_string),
            )?);
        }

        self.objects.push(ObjectModel {
            name,
            free_symbol: object.ffi_object_free().name().to_string(),
            clone_symbol: object.ffi_object_clone().name().to_string(),
            methods,
            docs: object.docstring().map(str::to_string),
        });
        Ok(())
    }

    fn collect_serialized(&mut self) -> Result<()> {
        let mut types: Vec<(Type, String)> = Vec::new();
        for record in &self.records {
            for field in &record.fields {
                types.push((
                    field.ty.clone(),
                    format!("{}.{}", record.name, field.rust_name),
                ));
            }
        }
        for enum_ in self.enums.iter().chain(self.errors.iter()) {
            for variant in &enum_.variants {
                for field in &variant.fields {
                    types.push((
                        field.ty.clone(),
                        format!("{}::{}.{}", enum_.name, variant.rust_name, field.rust_name),
                    ));
                }
            }
        }
        let callables = self
            .root_methods
            .iter()
            .chain(self.objects.iter().flat_map(|object| object.methods.iter()));
        for callable in callables {
            for arg in &callable.args {
                types.push((
                    arg.ty.clone(),
                    format!("{}({})", callable.rust_name, arg.rust_name),
                ));
            }
            if let Some((ty, _)) = &callable.returns {
                types.push((ty.clone(), format!("{} return", callable.rust_name)));
            }
        }

        for (ty, context) in types {
            self.register_serialized(&ty, &context)?;
        }
        Ok(())
    }

    fn register_serialized(&mut self, ty: &Type, context: &str) -> Result<()> {
        let mapped = map(ty, context)?;
        if self.serialized.contains_key(&mapped.ser) {
            return Ok(());
        }
        self.serialized
            .insert(mapped.ser.clone(), (ty.clone(), mapped));
        match ty {
            Type::Optional { inner_type } | Type::Sequence { inner_type } => {
                self.register_serialized(inner_type, context)?;
            }
            Type::Map { value_type, .. } => {
                self.register_serialized(value_type, context)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The record, enum or object a serializer name refers to, if any.
    pub fn object_named(&self, name: &str) -> Option<&ObjectModel> {
        self.objects.iter().find(|object| object.name == name)
    }
}

#[allow(clippy::too_many_arguments)]
fn model_callable(
    rust_name: &str,
    js_name: String,
    bridge_name: String,
    symbol: &str,
    arguments: Vec<uniffi_bindgen::interface::Argument>,
    return_type: Option<Type>,
    throws: Option<String>,
    has_status: bool,
    is_async: bool,
    kind: CallableKind,
    docs: Option<String>,
) -> Result<CallableModel> {
    if is_async {
        return Err(Error::UnsupportedFeature {
            context: rust_name.to_string(),
            feature: "Rust `async fn` (wrap the call in a Nitro Promise instead, see \
                      `async_methods` in uniffi.toml)"
                .to_string(),
        });
    }

    let mut args = Vec::new();
    for argument in arguments {
        let context = format!("{rust_name}({})", argument.name());
        let arg_type = argument.as_type();
        let mapped = map(&arg_type, &context)?;
        args.push(Arg {
            rust_name: argument.name().to_string(),
            js_name: lower_camel(argument.name()),
            ty: arg_type,
            mapped,
        });
    }

    let returns = match return_type {
        Some(ty) => {
            let mapped = map(&ty, &format!("{rust_name} return"))?;
            Some((ty, mapped))
        }
        None => None,
    };

    Ok(CallableModel {
        rust_name: rust_name.to_string(),
        js_name,
        bridge_name,
        symbol: symbol.to_string(),
        args,
        returns,
        throws,
        has_status,
        kind,
        docs,
    })
}

fn model_record(record: &Record) -> Result<RecordModel> {
    let name = pascal(record.name());
    let mut fields = Vec::new();
    for field in record.fields() {
        let context = format!("{name}.{}", field.name());
        fields.push(FieldModel {
            rust_name: field.name().to_string(),
            js_name: lower_camel(field.name()),
            ty: field.as_type(),
            mapped: map(&field.as_type(), &context)?,
        });
    }
    Ok(RecordModel {
        name,
        fields,
        docs: record.docstring().map(str::to_string),
    })
}

fn model_enum(enum_: &Enum, is_error: bool) -> Result<EnumModel> {
    let name = pascal(enum_.name());
    if !is_error && enum_.contains_variant_fields() {
        return Err(Error::UnsupportedFeature {
            context: name,
            feature: "enum variants with fields (a tagged union has no Nitro equivalent; \
                      model it as a record with an optional payload)"
                .to_string(),
        });
    }

    let mut variants = Vec::new();
    for variant in enum_.variants() {
        if variant.has_nameless_fields() {
            return Err(Error::UnsupportedFeature {
                context: format!("{name}::{}", variant.name()),
                feature: "tuple variants (name the fields so JavaScript can read them)".to_string(),
            });
        }
        let mut fields = Vec::new();
        for field in variant.fields() {
            let context = format!("{name}::{}.{}", variant.name(), field.name());
            fields.push(FieldModel {
                rust_name: field.name().to_string(),
                js_name: lower_camel(field.name()),
                ty: field.as_type(),
                mapped: map(&field.as_type(), &context)?,
            });
        }
        variants.push(VariantModel {
            rust_name: variant.name().to_string(),
            js_name: lower_camel(variant.name()),
            fields,
        });
    }

    Ok(EnumModel {
        name,
        variants,
        docs: enum_.docstring().map(str::to_string),
    })
}
