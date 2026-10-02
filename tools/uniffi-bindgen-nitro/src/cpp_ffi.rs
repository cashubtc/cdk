//! Emits the `extern "C"` view of the UniFFI ABI.
//!
//! Every symbol and signature comes from the metadata, so a renamed Rust
//! export can never silently keep calling the old symbol.

use std::collections::BTreeSet;

use uniffi_bindgen::interface::{FfiFunction, FfiType};
use uniffi_bindgen::ComponentInterface;

use crate::model::Model;
use crate::writer::{banner, Source};

/// The C ABI header the bridge includes.
pub fn header(model: &Model, ci: &ComponentInterface, crate_name: &str) -> String {
    let wanted = wanted_symbols(model);

    let mut out = Source::new();
    out.lines(&banner("//", crate_name));
    out.blank();
    out.line("#pragma once");
    out.blank();
    out.line("#include <cstdint>");
    out.blank();
    out.line(format!("namespace {}::ffi {{", model.cxx_namespace));
    out.blank();
    out.line("extern \"C\" {");
    out.blank();
    out.lines(
        "/// Mirrors `uniffi_core::RustBuffer`: a Vec<u8> handed over to the caller.\n\
         struct RustBuffer {\n\
        \x20 uint64_t capacity;\n\
        \x20 uint64_t len;\n\
        \x20 uint8_t* data;\n\
         };\n\
         \n\
         /// Mirrors `uniffi_core::ForeignBytes`: bytes the caller keeps alive.\n\
         struct ForeignBytes {\n\
        \x20 int32_t len;\n\
        \x20 const uint8_t* data;\n\
         };\n\
         \n\
         /// Mirrors `uniffi_core::RustCallStatus`.\n\
         struct RustCallStatus {\n\
        \x20 int8_t code;\n\
        \x20 RustBuffer errorBuf;\n\
         };\n\
         \n\
         /// `RustCallStatusCode` values.\n\
         enum RustCallStatusCode : int8_t {\n\
        \x20 RUST_CALL_SUCCESS = 0,\n\
        \x20 RUST_CALL_ERROR = 1,\n\
        \x20 RUST_CALL_UNEXPECTED_ERROR = 2,\n\
        \x20 RUST_CALL_CANCELLED = 3,\n\
         };",
    );
    out.blank();

    let mut emitted = BTreeSet::new();
    for function in ci.iter_ffi_function_definitions() {
        if !wanted.contains(function.name()) || !emitted.insert(function.name().to_string()) {
            continue;
        }
        if let Some(decl) = declaration(&function) {
            out.line(decl);
        }
    }

    out.blank();
    out.line("} // extern \"C\"");
    out.blank();
    out.line(format!("}} // namespace {}::ffi", model.cxx_namespace));
    out.finish()
}

fn wanted_symbols(model: &Model) -> BTreeSet<String> {
    let mut wanted = BTreeSet::new();
    wanted.insert(model.rustbuffer_alloc.clone());
    wanted.insert(model.rustbuffer_free.clone());
    wanted.insert(model.rustbuffer_from_bytes.clone());
    wanted.insert(model.contract_version_symbol.clone());
    for (symbol, _) in &model.checksums {
        wanted.insert(symbol.clone());
    }
    for callable in &model.root_methods {
        wanted.insert(callable.symbol.clone());
    }
    for object in &model.objects {
        wanted.insert(object.free_symbol.clone());
        wanted.insert(object.clone_symbol.clone());
        for method in &object.methods {
            wanted.insert(method.symbol.clone());
        }
    }
    wanted
}

fn declaration(function: &FfiFunction) -> Option<String> {
    let mut params: Vec<String> = Vec::new();
    for argument in function.arguments() {
        params.push(format!(
            "{} {}",
            c_type(&argument.type_())?,
            argument.name()
        ));
    }
    if function.has_rust_call_status_arg() {
        params.push("RustCallStatus* uniffiOutStatus".to_string());
    }
    let params = if params.is_empty() {
        "void".to_string()
    } else {
        params.join(", ")
    };
    let returns = match function.return_type() {
        Some(ty) => c_type(ty)?,
        None => "void".to_string(),
    };
    Some(format!("{returns} {}({params});", function.name()))
}

fn c_type(ty: &FfiType) -> Option<String> {
    let text = match ty {
        FfiType::UInt8 => "uint8_t",
        FfiType::Int8 => "int8_t",
        FfiType::UInt16 => "uint16_t",
        FfiType::Int16 => "int16_t",
        FfiType::UInt32 => "uint32_t",
        FfiType::Int32 => "int32_t",
        FfiType::UInt64 | FfiType::Handle => "uint64_t",
        FfiType::Int64 => "int64_t",
        FfiType::Float32 => "float",
        FfiType::Float64 => "double",
        FfiType::RustBuffer(_) => "RustBuffer",
        FfiType::ForeignBytes => "ForeignBytes",
        FfiType::RustCallStatus => "RustCallStatus",
        FfiType::VoidPointer => "void*",
        FfiType::Reference(inner) => return Some(format!("const {}*", c_type(inner)?)),
        FfiType::MutReference(inner) => return Some(format!("{}*", c_type(inner)?)),
        FfiType::Callback(_) | FfiType::Struct(_) => return None,
    };
    Some(text.to_string())
}
