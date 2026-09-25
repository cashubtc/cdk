//! The single place where a UniFFI type becomes a TypeScript, Nitro C++ or
//! bridge C++ type.

use uniffi_bindgen::interface::Type;

use crate::error::{Error, Result};
use crate::naming::pascal;

/// How one UniFFI type is spelled in each language this generator emits.
#[derive(Debug, Clone)]
pub struct Mapped {
    /// TypeScript spelling used in the `.nitro.ts` spec.
    pub ts: String,
    /// C++ spelling nitrogen will use for that TypeScript type.
    pub nitro: String,
    /// C++ spelling used by the plain bridge layer.
    pub bridge: String,
    /// Suffix identifying the type in generated serializer names.
    pub ser: String,
}

/// Map a UniFFI type, reporting the argument or field it came from on failure.
pub fn map(ty: &Type, context: &str) -> Result<Mapped> {
    let mapped = match ty {
        Type::Boolean => simple("boolean", "bool", "bool", "Bool"),
        Type::Int8 => simple("number", "double", "int8_t", "I8"),
        Type::UInt8 => simple("number", "double", "uint8_t", "U8"),
        Type::Int16 => simple("number", "double", "int16_t", "I16"),
        Type::UInt16 => simple("number", "double", "uint16_t", "U16"),
        Type::Int32 => simple("number", "double", "int32_t", "I32"),
        Type::UInt32 => simple("number", "double", "uint32_t", "U32"),
        Type::Int64 => simple("Int64", "int64_t", "int64_t", "I64"),
        Type::UInt64 => simple("UInt64", "uint64_t", "uint64_t", "U64"),
        Type::Float32 => simple("number", "double", "float", "F32"),
        Type::Float64 => simple("number", "double", "double", "F64"),
        Type::String => simple("string", "std::string", "std::string", "String"),
        Type::Bytes => simple(
            "ArrayBuffer",
            "std::shared_ptr<ArrayBuffer>",
            "std::vector<uint8_t>",
            "Bytes",
        ),
        Type::Optional { inner_type } => {
            let inner = map(inner_type, context)?;
            if matches!(**inner_type, Type::Optional { .. }) {
                return Err(Error::UnsupportedType {
                    context: context.to_string(),
                    type_name: "Option<Option<T>>".to_string(),
                    hint: "; JavaScript cannot tell the two levels of absence apart".to_string(),
                });
            }
            Mapped {
                ts: format!("{} | undefined", inner.ts),
                nitro: format!("std::optional<{}>", inner.nitro),
                bridge: format!("std::optional<{}>", inner.bridge),
                ser: format!("Opt{}", inner.ser),
            }
        }
        Type::Sequence { inner_type } => {
            let inner = map(inner_type, context)?;
            let ts = if inner.ts.contains(' ') {
                format!("({})[]", inner.ts)
            } else {
                format!("{}[]", inner.ts)
            };
            Mapped {
                ts,
                nitro: format!("std::vector<{}>", inner.nitro),
                bridge: format!("std::vector<{}>", inner.bridge),
                ser: format!("Seq{}", inner.ser),
            }
        }
        Type::Map {
            key_type,
            value_type,
        } => {
            if !matches!(**key_type, Type::String) {
                return Err(Error::UnsupportedType {
                    context: context.to_string(),
                    type_name: format!("HashMap with {key_type:?} keys"),
                    hint: "; JavaScript object keys are strings, use a sequence of records instead"
                        .to_string(),
                });
            }
            let value = map(value_type, context)?;
            Mapped {
                ts: format!("Record<string, {}>", value.ts),
                nitro: format!("std::unordered_map<std::string, {}>", value.nitro),
                bridge: format!("std::unordered_map<std::string, {}>", value.bridge),
                ser: format!("MapString{}", value.ser),
            }
        }
        Type::Record { name, .. } => {
            let name = pascal(name);
            Mapped {
                ts: name.clone(),
                nitro: name.clone(),
                bridge: format!("bridge::{name}"),
                ser: name,
            }
        }
        Type::Enum { name, .. } => {
            let name = pascal(name);
            Mapped {
                ts: name.clone(),
                nitro: name.clone(),
                bridge: format!("bridge::{name}"),
                ser: name,
            }
        }
        Type::Object { name, .. } => {
            let name = pascal(name);
            Mapped {
                ts: name.clone(),
                nitro: format!("std::shared_ptr<Hybrid{name}Spec>"),
                bridge: format!("std::shared_ptr<bridge::{name}>"),
                ser: name,
            }
        }
        Type::Custom { builtin, .. } => map(builtin, context)?,
        other => {
            return Err(Error::UnsupportedType {
                context: context.to_string(),
                type_name: format!("{other:?}"),
                hint: String::new(),
            })
        }
    };
    Ok(mapped)
}

/// True when the Nitro C++ signature passes this type as `const T&`.
///
/// Mirrors nitrogen's own rule so generated overrides match the spec exactly.
pub fn passed_by_reference(ty: &Type) -> bool {
    match ty {
        Type::Boolean
        | Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Int64
        | Type::UInt64
        | Type::Float32
        | Type::Float64
        | Type::Enum { .. } => false,
        Type::Custom { builtin, .. } => passed_by_reference(builtin),
        _ => true,
    }
}

impl Mapped {
    /// The bridge spelling as written inside the `bridge` namespace itself.
    pub fn bridge_local(&self) -> String {
        self.bridge.replace("bridge::", "")
    }
}

fn simple(ts: &str, nitro: &str, bridge: &str, ser: &str) -> Mapped {
    Mapped {
        ts: ts.to_string(),
        nitro: nitro.to_string(),
        bridge: bridge.to_string(),
        ser: ser.to_string(),
    }
}
