//! Emits the plain C++ bridge: types, serializers and one function per export.
//!
//! Nothing here depends on React Native, so the layer that actually crosses the
//! UniFFI ABI can be compiled and tested with a bare C++ toolchain.

use std::collections::{BTreeMap, BTreeSet};

use uniffi_bindgen::interface::Type;

use crate::cpp_runtime;
use crate::error::{Error, Result};
use crate::model::{CallableModel, EnumModel, Model, ObjectModel, RecordModel};
use crate::naming::pascal;
use crate::writer::{banner, Source};

/// The bridge header.
pub fn header(model: &Model, crate_name: &str) -> Result<String> {
    let mut out = Source::new();
    out.lines(&banner("//", crate_name));
    out.blank();
    out.line("#pragma once");
    out.blank();
    out.line("#include <cstdint>");
    out.line("#include <memory>");
    out.line("#include <optional>");
    out.line("#include <stdexcept>");
    out.line("#include <string>");
    out.line("#include <unordered_map>");
    out.line("#include <vector>");
    out.blank();
    out.line(format!("namespace {}::bridge {{", model.cxx_namespace));
    out.blank();

    out.line("/// ABI contract version the Rust library was generated against.");
    out.line(format!(
        "inline constexpr uint32_t kUniffiContractVersion = {};",
        model.contract_version
    ));
    out.blank();
    out.line("/// Aborts at startup if the loaded library is not the one this was generated from.");
    out.line("void assertAbiCompatible();");
    out.blank();

    for error in &model.errors {
        emit_error_type(&mut out, error)?;
    }
    for enum_ in &model.enums {
        emit_enum(&mut out, enum_);
    }
    for record in order_records(model)? {
        emit_record_declaration(&mut out, record);
    }
    for object in &model.objects {
        emit_object_declaration(&mut out, model, object);
    }

    out.line("// Free functions and object constructors.");
    for callable in &model.root_methods {
        docs(&mut out, callable.docs.as_deref());
        out.line(format!("{};", signature(model, callable, None)));
    }

    out.blank();
    out.line(format!("}} // namespace {}::bridge", model.cxx_namespace));
    Ok(out.finish())
}

/// The bridge implementation.
pub fn source(
    model: &Model,
    crate_name: &str,
    header_name: &str,
    ffi_header: &str,
) -> Result<String> {
    let mut out = Source::new();
    out.lines(&banner("//", crate_name));
    out.blank();
    out.line(format!("#include \"{header_name}\""));
    out.line(format!("#include \"{ffi_header}\""));
    out.blank();
    out.line("#include <cstdio>");
    out.line("#include <cstring>");
    out.blank();
    out.line(format!("namespace {}::bridge {{", model.cxx_namespace));
    out.line("namespace {");
    out.blank();
    out.lines(&cpp_runtime::helpers(
        &model.rustbuffer_free,
        &model.rustbuffer_from_bytes,
    ));
    out.blank();

    emit_serializer_declarations(&mut out, model)?;
    out.blank();
    emit_serializer_definitions(&mut out, model)?;
    out.blank();
    for error in &model.errors {
        emit_error_thrower(&mut out, error)?;
    }

    out.line("} // namespace");
    out.blank();

    emit_abi_check(&mut out, model);

    for object in &model.objects {
        emit_object_definition(&mut out, model, object)?;
    }
    for callable in &model.root_methods {
        emit_callable_definition(&mut out, model, callable, None)?;
    }

    out.line(format!("}} // namespace {}::bridge", model.cxx_namespace));
    Ok(out.finish())
}

fn emit_abi_check(out: &mut Source, model: &Model) {
    out.line("void assertAbiCompatible() {");
    out.indented(|out| {
        out.line(format!(
            "uint32_t version = ffi::{}();",
            model.contract_version_symbol
        ));
        out.line("if (version != kUniffiContractVersion) {");
        out.indented(|out| {
            out.line("throw std::runtime_error(\"uniffi ABI mismatch: rebuild the bindings\");");
        });
        out.line("}");
        for (symbol, expected) in &model.checksums {
            out.line(format!(
                "if (ffi::{symbol}() != {expected}) {{ throw std::runtime_error(\"uniffi checksum mismatch for {symbol}: rebuild the bindings\"); }}"
            ));
        }
    });
    out.line("}");
    out.blank();
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

fn emit_error_type(out: &mut Source, error: &EnumModel) -> Result<()> {
    for variant in &error.variants {
        for field in &variant.fields {
            json_kind(&field.ty).ok_or_else(|| Error::UnsupportedType {
                context: format!("{}::{}.{}", error.name, variant.rust_name, field.rust_name),
                type_name: format!("{:?}", field.ty),
                hint: "; error fields travel to JavaScript as JSON, so use scalars, \
                       strings, bytes, plain enums, options or sequences of those"
                    .to_string(),
            })?;
        }
    }

    docs(out, error.docs.as_deref());
    out.line(format!("enum class {}Kind : int32_t {{", error.name));
    out.indented(|out| {
        for (index, variant) in error.variants.iter().enumerate() {
            out.line(format!("{} = {},", variant.rust_name, index + 1));
        }
    });
    out.line("};");
    out.blank();

    out.line(format!("/// The Rust `{}` as a C++ exception.", error.name));
    out.line("///");
    out.line("/// `what()` carries the whole variant as JSON so the TypeScript layer can");
    out.line("/// rebuild a typed error; `kind()` is there for C++ callers.");
    out.line(format!(
        "class {} final : public std::runtime_error {{",
        error.name
    ));
    out.line("public:");
    out.indented(|out| {
        out.line(format!(
            "{}({}Kind kind, const std::string& payload)",
            error.name, error.name
        ));
        out.indented(|out| {
            out.line(": std::runtime_error(payload), kind_(kind) {}");
        });
        out.blank();
        out.line(format!(
            "{}Kind kind() const noexcept {{ return kind_; }}",
            error.name
        ));
    });
    out.blank();
    out.line("private:");
    out.indented(|out| {
        out.line(format!("{}Kind kind_;", error.name));
    });
    out.line("};");
    out.blank();
    Ok(())
}

fn emit_enum(out: &mut Source, enum_: &EnumModel) {
    docs(out, enum_.docs.as_deref());
    out.line(format!("enum class {} : int32_t {{", enum_.name));
    out.indented(|out| {
        for (index, variant) in enum_.variants.iter().enumerate() {
            out.line(format!("{} = {},", variant.rust_name, index + 1));
        }
    });
    out.line("};");
    out.blank();
}

fn emit_record_declaration(out: &mut Source, record: &RecordModel) {
    docs(out, record.docs.as_deref());
    out.line(format!("struct {} final {{", record.name));
    out.indented(|out| {
        for field in &record.fields {
            out.line(format!(
                "{} {};",
                field.mapped.bridge_local(),
                field.js_name
            ));
        }
    });
    out.line("};");
    out.blank();
}

fn emit_object_declaration(out: &mut Source, model: &Model, object: &ObjectModel) {
    docs(out, object.docs.as_deref());
    out.line(format!(
        "/// Owns the UniFFI handle for the Rust `{}`.",
        object.name
    ));
    out.line(format!("class {} final {{", object.name));
    out.line("public:");
    out.indented(|out| {
        out.line(format!(
            "explicit {}(uint64_t handle) noexcept : handle_(handle) {{}}",
            object.name
        ));
        out.line(format!("~{}() {{ close(); }}", object.name));
        out.line(format!("{0}(const {0}&) = delete;", object.name));
        out.line(format!("{0}& operator=(const {0}&) = delete;", object.name));
        out.line(format!(
            "{0}({0}&& other) noexcept : handle_(other.handle_) {{ other.handle_ = 0; }}",
            object.name
        ));
        out.blank();
        out.line("/// Drop the Rust reference. Safe to call more than once.");
        out.line("void close() noexcept;");
        out.blank();
        out.line("/// The raw handle, or zero once closed.");
        out.line("uint64_t handle() const noexcept { return handle_; }");
        out.blank();
        out.line("/// A fresh owned handle.");
        out.line("///");
        out.line("/// UniFFI consumes the receiver of every method call, so each call");
        out.line("/// hands it a clone rather than this object's own reference.");
        out.line("uint64_t cloneHandle() const;");
        out.blank();
        for method in &object.methods {
            docs(out, method.docs.as_deref());
            out.line(format!(
                "{};",
                declaration(model, method, Some(&object.name))
            ));
        }
    });
    out.blank();
    out.line("private:");
    out.indented(|out| {
        out.line("uint64_t handle_;");
    });
    out.line("};");
    out.blank();
}

fn emit_object_definition(out: &mut Source, model: &Model, object: &ObjectModel) -> Result<()> {
    out.line(format!("uint64_t {}::cloneHandle() const {{", object.name));
    out.indented(|out| {
        out.line("if (handle_ == 0) {");
        out.indented(|out| {
            out.line("throw std::logic_error(\"use of a native object after close()\");");
        });
        out.line("}");
        out.line("ffi::RustCallStatus status;");
        out.line("status.code = 0;");
        out.line("status.errorBuf = ffi::RustBuffer{0, 0, nullptr};");
        out.line(format!(
            "uint64_t cloned = ffi::{}(handle_, &status);",
            object.clone_symbol
        ));
        out.line("checkUnexpected(status);");
        out.line("return cloned;");
    });
    out.line("}");
    out.blank();

    out.line(format!("void {}::close() noexcept {{", object.name));
    out.indented(|out| {
        out.line("if (handle_ == 0) {");
        out.indented(|out| {
            out.line("return;");
        });
        out.line("}");
        out.line("ffi::RustCallStatus status;");
        out.line("status.code = 0;");
        out.line("status.errorBuf = ffi::RustBuffer{0, 0, nullptr};");
        out.line(format!("ffi::{}(handle_, &status);", object.free_symbol));
        out.line("handle_ = 0;");
    });
    out.line("}");
    out.blank();

    for method in &object.methods {
        emit_callable_definition(out, model, method, Some(object))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Signatures and bodies
// ---------------------------------------------------------------------------

fn signature(model: &Model, callable: &CallableModel, owner: Option<&ObjectModel>) -> String {
    signature_with(
        model,
        callable,
        owner.map(|object| object.name.as_str()),
        true,
    )
}

/// The same signature as written inside a class body, without the class prefix.
fn declaration(model: &Model, callable: &CallableModel, owner: Option<&str>) -> String {
    signature_with(model, callable, owner, false)
}

fn signature_with(
    _model: &Model,
    callable: &CallableModel,
    owner: Option<&str>,
    qualified: bool,
) -> String {
    let params = callable
        .args
        .iter()
        .map(|arg| {
            let ty = arg.mapped.bridge_local();
            if crate::typemap::passed_by_reference(&arg.ty) {
                format!("const {ty}& {}", arg.js_name)
            } else {
                format!("{ty} {}", arg.js_name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let returns = return_type(callable);
    match (owner, qualified) {
        (Some(owner), true) => format!("{returns} {owner}::{}({params}) const", callable.js_name),
        (Some(_), false) => format!("{returns} {}({params}) const", callable.js_name),
        (None, _) => format!("{returns} {}({params})", callable.js_name),
    }
}

fn return_type(callable: &CallableModel) -> String {
    match &callable.returns {
        None => "void".to_string(),
        Some((_, mapped)) => mapped.bridge_local(),
    }
}

fn emit_callable_definition(
    out: &mut Source,
    model: &Model,
    callable: &CallableModel,
    owner: Option<&ObjectModel>,
) -> Result<()> {
    out.line(format!("{} {{", signature(model, callable, owner)));
    let mut body = Source::new();
    emit_call_body(&mut body, model, callable, owner)?;
    out.indented(|out| {
        out.lines(&body.finish());
    });
    out.line("}");
    out.blank();
    Ok(())
}

fn emit_call_body(
    out: &mut Source,
    _model: &Model,
    callable: &CallableModel,
    owner: Option<&ObjectModel>,
) -> Result<()> {
    let mut call_args: Vec<String> = Vec::new();
    if owner.is_some() {
        // The receiver is consumed by the call, so hand over a clone.
        call_args.push("cloneHandle()".to_string());
    }
    for (index, arg) in callable.args.iter().enumerate() {
        let lowered = format!("lowered{index}");
        out.line(format!(
            "auto {lowered} = {};",
            lower_expr(&arg.ty, &arg.js_name, &arg.mapped.ser)?
        ));
        call_args.push(lowered);
    }

    if callable.has_status {
        out.line("ffi::RustCallStatus status;");
        out.line("status.code = 0;");
        out.line("status.errorBuf = ffi::RustBuffer{0, 0, nullptr};");
        call_args.push("&status".to_string());
    }

    let call = format!("ffi::{}({})", callable.symbol, call_args.join(", "));
    match &callable.returns {
        Some(_) => out.line(format!("auto rawResult = {call};")),
        None => out.line(format!("{call};")),
    };

    if callable.has_status {
        if let Some(error) = &callable.throws {
            out.line("if (status.code == ffi::RUST_CALL_ERROR) {");
            out.indented(|out| {
                out.line(format!("throw{error}(status.errorBuf);"));
            });
            out.line("}");
        }
        out.line("checkUnexpected(status);");
    }

    let Some((ty, _)) = &callable.returns else {
        return Ok(());
    };

    let mapped = crate::typemap::map(ty, &callable.rust_name)?;
    out.line(format!(
        "return {};",
        lift_expr(ty, "rawResult", &mapped.ser)?
    ));
    Ok(())
}

/// C++ expression turning a bridge value into what the FFI symbol expects.
fn lower_expr(ty: &Type, value: &str, ser: &str) -> Result<String> {
    Ok(match ty {
        Type::Boolean => format!("static_cast<int8_t>({value} ? 1 : 0)"),
        Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Int64
        | Type::UInt64
        | Type::Float32
        | Type::Float64 => value.to_string(),
        Type::String => format!("stringToBuffer({value})"),
        // `Vec<u8>` lowers through the generic sequence path, so the buffer
        // carries an i32 length prefix; a `String` does not.
        Type::Bytes => format!("lower{ser}({value})"),
        Type::Enum { .. }
        | Type::Record { .. }
        | Type::Optional { .. }
        | Type::Sequence { .. }
        | Type::Map { .. } => format!("lower{ser}({value})"),
        Type::Object { name, .. } => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("object argument `{name}`"),
                hint: "; pass the data the object holds instead, or add a method on the object"
                    .to_string(),
            })
        }
        Type::Custom { builtin, .. } => lower_expr(builtin, value, ser)?,
        other => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("{other:?}"),
                hint: String::new(),
            })
        }
    })
}

/// C++ expression turning what the FFI symbol returned into a bridge value.
fn lift_expr(ty: &Type, value: &str, ser: &str) -> Result<String> {
    Ok(match ty {
        Type::Boolean => format!("({value} != 0)"),
        Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Int64
        | Type::UInt64
        | Type::Float32
        | Type::Float64 => value.to_string(),
        Type::String => format!("consumeBufferAsString({value})"),
        Type::Bytes => format!("lift{ser}({value})"),
        Type::Enum { .. }
        | Type::Record { .. }
        | Type::Optional { .. }
        | Type::Sequence { .. }
        | Type::Map { .. } => format!("lift{ser}({value})"),
        Type::Object { name, .. } => {
            format!("std::make_shared<{}>({value})", pascal(name))
        }
        Type::Custom { builtin, .. } => lift_expr(builtin, value, ser)?,
        other => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("{other:?}"),
                hint: String::new(),
            })
        }
    })
}

// ---------------------------------------------------------------------------
// Serializers
// ---------------------------------------------------------------------------

fn emit_serializer_declarations(out: &mut Source, model: &Model) -> Result<()> {
    out.line("// Forward declarations so serializers can reference each other freely.");
    for (ser, (ty, mapped)) in &model.serialized {
        if !needs_serializer(ty) {
            continue;
        }
        let cpp = mapped.bridge_local();
        out.line(format!("{cpp} read{ser}(BufferReader& reader);"));
        out.line(format!(
            "void write{ser}(BufferWriter& writer, const {cpp}& value);"
        ));
        if is_top_level(ty) {
            out.line(format!("RustBuffer lower{ser}(const {cpp}& value);"));
            out.line(format!("{cpp} lift{ser}(RustBuffer buffer);"));
        }
    }
    Ok(())
}

fn emit_serializer_definitions(out: &mut Source, model: &Model) -> Result<()> {
    for (ser, (ty, mapped)) in &model.serialized {
        if !needs_serializer(ty) {
            continue;
        }
        let cpp = mapped.bridge_local();

        out.line(format!("{cpp} read{ser}(BufferReader& reader) {{"));
        out.indented(|out| {
            out.lines(&read_body(model, ty, ser));
        });
        out.line("}");
        out.blank();

        out.line(format!(
            "void write{ser}(BufferWriter& writer, const {cpp}& value) {{"
        ));
        out.indented(|out| {
            out.lines(&write_body(model, ty));
        });
        out.line("}");
        out.blank();

        if is_top_level(ty) {
            out.line(format!("RustBuffer lower{ser}(const {cpp}& value) {{"));
            out.indented(|out| {
                out.line("BufferWriter writer;");
                out.line(format!("write{ser}(writer, value);"));
                out.line("return bytesToBuffer(writer.bytes());");
            });
            out.line("}");
            out.blank();

            out.line(format!("{cpp} lift{ser}(RustBuffer buffer) {{"));
            out.indented(|out| {
                out.line("std::vector<uint8_t> bytes = consumeBuffer(buffer);");
                out.line("BufferReader reader(bytes.data(), bytes.size());");
                out.line(format!("{cpp} value = read{ser}(reader);"));
                out.line("reader.finish();");
                out.line("return value;");
            });
            out.line("}");
            out.blank();
        }
    }
    Ok(())
}

fn read_body(model: &Model, ty: &Type, ser: &str) -> String {
    let mut out = Source::new();
    match ty {
        Type::Boolean => out.line("return reader.readBool();"),
        Type::Int8 => out.line("return reader.readI8();"),
        Type::UInt8 => out.line("return reader.readU8();"),
        Type::Int16 => out.line("return reader.readI16();"),
        Type::UInt16 => out.line("return reader.readU16();"),
        Type::Int32 => out.line("return reader.readI32();"),
        Type::UInt32 => out.line("return reader.readU32();"),
        Type::Int64 => out.line("return reader.readI64();"),
        Type::UInt64 => out.line("return reader.readU64();"),
        Type::Float32 => out.line("return reader.readF32();"),
        Type::Float64 => out.line("return reader.readF64();"),
        Type::String => out.line("return reader.readString();"),
        Type::Bytes => out.line("return reader.readBytes();"),
        Type::Optional { inner_type } => {
            let inner = child(inner_type);
            out.line("if (reader.readI8() == 0) {");
            out.indented(|out| {
                out.line("return std::nullopt;");
            });
            out.line("}");
            out.line(format!("return read{inner}(reader);"))
        }
        Type::Sequence { inner_type } => {
            let inner = child(inner_type);
            let cpp = crate::typemap::map(inner_type, "sequence")
                .map(|m| m.bridge_local())
                .unwrap_or_else(|_| "auto".to_string());
            out.line("size_t count = reader.readLength();");
            out.line(format!("std::vector<{cpp}> items;"));
            out.line("items.reserve(count);");
            out.line("for (size_t i = 0; i < count; i++) {");
            out.indented(|out| {
                out.line(format!("items.push_back(read{inner}(reader));"));
            });
            out.line("}");
            out.line("return items;")
        }
        Type::Map { value_type, .. } => {
            let inner = child(value_type);
            let cpp = crate::typemap::map(value_type, "map")
                .map(|m| m.bridge_local())
                .unwrap_or_else(|_| "auto".to_string());
            out.line("size_t count = reader.readLength();");
            out.line(format!("std::unordered_map<std::string, {cpp}> entries;"));
            out.line("entries.reserve(count);");
            out.line("for (size_t i = 0; i < count; i++) {");
            out.indented(|out| {
                out.line("std::string key = reader.readString();");
                out.line(format!(
                    "entries.emplace(std::move(key), read{inner}(reader));"
                ));
            });
            out.line("}");
            out.line("return entries;")
        }
        Type::Record { name, .. } => {
            let record = model
                .records
                .iter()
                .find(|record| record.name == pascal(name));
            out.line(format!("{} value;", pascal(name)));
            if let Some(record) = record {
                for field in &record.fields {
                    let inner = child(&field.ty);
                    out.line(format!("value.{} = read{inner}(reader);", field.js_name));
                }
            }
            out.line("return value;")
        }
        Type::Enum { name, .. } => {
            let enum_ = model.enums.iter().find(|e| e.name == pascal(name));
            out.line("int32_t tag = reader.readI32();");
            out.line("switch (tag) {");
            out.indented(|out| {
                if let Some(enum_) = enum_ {
                    for (index, variant) in enum_.variants.iter().enumerate() {
                        out.line(format!(
                            "case {}: return {}::{};",
                            index + 1,
                            enum_.name,
                            variant.rust_name
                        ));
                    }
                }
                out.line("default:");
                out.indented(|out| {
                    out.line(format!(
                        "throw std::runtime_error(\"unknown {ser} variant tag\");"
                    ));
                });
            });
            out.line("}")
        }
        Type::Object { name, .. } => out.line(format!(
            "return std::make_shared<{}>(reader.readU64());",
            pascal(name)
        )),
        _ => out.line("throw std::runtime_error(\"unsupported type\");"),
    };
    out.finish()
}

fn write_body(model: &Model, ty: &Type) -> String {
    let mut out = Source::new();
    match ty {
        Type::Boolean => out.line("writer.writeBool(value);"),
        Type::Int8 => out.line("writer.writeI8(value);"),
        Type::UInt8 => out.line("writer.writeU8(value);"),
        Type::Int16 => out.line("writer.writeI16(value);"),
        Type::UInt16 => out.line("writer.writeU16(value);"),
        Type::Int32 => out.line("writer.writeI32(value);"),
        Type::UInt32 => out.line("writer.writeU32(value);"),
        Type::Int64 => out.line("writer.writeI64(value);"),
        Type::UInt64 => out.line("writer.writeU64(value);"),
        Type::Float32 => out.line("writer.writeF32(value);"),
        Type::Float64 => out.line("writer.writeF64(value);"),
        Type::String => out.line("writer.writeString(value);"),
        Type::Bytes => out.line("writer.writeBytes(value);"),
        Type::Optional { inner_type } => {
            let inner = child(inner_type);
            out.line("if (!value.has_value()) {");
            out.indented(|out| {
                out.line("writer.writeI8(0);");
                out.line("return;");
            });
            out.line("}");
            out.line("writer.writeI8(1);");
            out.line(format!("write{inner}(writer, *value);"))
        }
        Type::Sequence { inner_type } => {
            let inner = child(inner_type);
            out.line("writer.writeLength(value.size());");
            out.line("for (const auto& item : value) {");
            out.indented(|out| {
                out.line(format!("write{inner}(writer, item);"));
            });
            out.line("}")
        }
        Type::Map { value_type, .. } => {
            let inner = child(value_type);
            out.line("writer.writeLength(value.size());");
            out.line("for (const auto& entry : value) {");
            out.indented(|out| {
                out.line("writer.writeString(entry.first);");
                out.line(format!("write{inner}(writer, entry.second);"));
            });
            out.line("}")
        }
        Type::Record { name, .. } => {
            let record = model
                .records
                .iter()
                .find(|record| record.name == pascal(name));
            if let Some(record) = record {
                for field in &record.fields {
                    let inner = child(&field.ty);
                    out.line(format!("write{inner}(writer, value.{});", field.js_name));
                }
            }
            out.line("")
        }
        Type::Enum { .. } => out.line("writer.writeI32(static_cast<int32_t>(value));"),
        Type::Object { .. } => out.line("writer.writeU64(value->cloneHandle());"),
        _ => out.line("throw std::runtime_error(\"unsupported type\");"),
    };
    out.finish()
}

fn emit_error_thrower(out: &mut Source, error: &EnumModel) -> Result<()> {
    out.line(format!(
        "[[noreturn]] void throw{}(RustBuffer buffer) {{",
        error.name
    ));
    out.indented(|out| {
        out.line("std::vector<uint8_t> bytes = consumeBuffer(buffer);");
        out.line("BufferReader reader(bytes.data(), bytes.size());");
        out.line("int32_t tag = reader.readI32();");
        out.line("std::string payload;");
        out.line("switch (tag) {");
        out.indented(|out| {
            for (index, variant) in error.variants.iter().enumerate() {
                out.line(format!("case {}: {{", index + 1));
                out.indented(|out| {
                    for field in &variant.fields {
                        let ser = child(&field.ty);
                        let cpp = field.mapped.bridge_local();
                        out.line(format!("{cpp} {} = read{ser}(reader);", field.js_name));
                    }
                    emit_error_payload(out, error, variant);
                    out.line("break;");
                });
                out.line("}");
            }
            out.line("default:");
            out.indented(|out| {
                out.line(format!(
                    "payload = \"{{\\\"type\\\":\\\"{}\\\",\\\"kind\\\":\\\"Unknown\\\",\\\"message\\\":\\\"unknown variant\\\",\\\"fields\\\":{{}}}}\";",
                    error.name
                ));
            });
        });
        out.line("}");
        out.line(format!(
            "throw {}(static_cast<{}Kind>(tag), \"uniffi-nitro-error:\" + payload);",
            error.name, error.name
        ));
    });
    out.line("}");
    out.blank();
    Ok(())
}

fn emit_error_payload(out: &mut Source, error: &EnumModel, variant: &crate::model::VariantModel) {
    out.line("std::string fields = \"{\";");
    for (index, field) in variant.fields.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        out.line(format!(
            "fields += \"{comma}\\\"{}\\\":\" + {};",
            field.js_name,
            json_value_expr(&field.ty, &field.js_name)
        ));
    }
    out.line("fields += \"}\";");

    let mut message = format!("std::string(\"{}\")", variant.rust_name);
    if !variant.fields.is_empty() {
        message.push_str(" + \"(\"");
        for (index, field) in variant.fields.iter().enumerate() {
            let sep = if index == 0 { "" } else { ", " };
            message.push_str(&format!(
                " + \"{sep}{}=\" + {}",
                field.js_name,
                json_plain_expr(&field.ty, &field.js_name)
            ));
        }
        message.push_str(" + \")\"");
    }
    out.line(format!("std::string message = {message};"));
    out.line(format!(
        "payload = std::string(\"{{\\\"type\\\":\\\"{}\\\",\\\"kind\\\":\\\"{}\\\",\\\"message\\\":\\\"\") + jsonEscape(message) + \"\\\",\\\"fields\\\":\" + fields + \"}}\";",
        error.name, variant.rust_name
    ));
}

/// How an error field is rendered inside the JSON payload.
fn json_kind(ty: &Type) -> Option<()> {
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
        | Type::String
        | Type::Bytes
        | Type::Enum { .. } => Some(()),
        Type::Optional { inner_type } | Type::Sequence { inner_type } => json_kind(inner_type),
        Type::Custom { builtin, .. } => json_kind(builtin),
        _ => None,
    }
}

fn json_value_expr(ty: &Type, value: &str) -> String {
    match ty {
        Type::Boolean => format!("std::string({value} ? \"true\" : \"false\")"),
        Type::Int64 | Type::UInt64 => {
            format!("(\"\\\"\" + std::to_string({value}) + \"\\\"\")")
        }
        Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Float32
        | Type::Float64 => format!("std::to_string({value})"),
        Type::String => format!("(\"\\\"\" + jsonEscape({value}) + \"\\\"\")"),
        Type::Bytes => format!("(\"\\\"\" + toHexString({value}) + \"\\\"\")"),
        Type::Enum { .. } => format!(
            "(\"\\\"\" + std::to_string(static_cast<int32_t>({value})) + \"\\\"\")"
        ),
        Type::Optional { inner_type } => format!(
            "({value}.has_value() ? {} : std::string(\"null\"))",
            json_value_expr(inner_type, &format!("(*{value})"))
        ),
        Type::Sequence { inner_type } => format!(
            "([&]{{ std::string out = \"[\"; bool first = true; for (const auto& item : {value}) {{ if (!first) out += \",\"; first = false; out += {}; }} out += \"]\"; return out; }}())",
            json_value_expr(inner_type, "item")
        ),
        Type::Custom { builtin, .. } => json_value_expr(builtin, value),
        _ => "std::string(\"null\")".to_string(),
    }
}

fn json_plain_expr(ty: &Type, value: &str) -> String {
    match ty {
        Type::String => value.to_string(),
        Type::Bytes => format!("toHexString({value})"),
        Type::Boolean => format!("std::string({value} ? \"true\" : \"false\")"),
        Type::Optional { inner_type } => format!(
            "({value}.has_value() ? {} : std::string(\"none\"))",
            json_plain_expr(inner_type, &format!("(*{value})"))
        ),
        Type::Enum { .. } => format!("std::to_string(static_cast<int32_t>({value}))"),
        Type::Sequence { .. } => "std::string(\"[..]\")".to_string(),
        Type::Custom { builtin, .. } => json_plain_expr(builtin, value),
        _ => format!("std::to_string({value})"),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn child(ty: &Type) -> String {
    crate::typemap::map(ty, "nested")
        .map(|mapped| mapped.ser)
        .unwrap_or_else(|_| "Unsupported".to_string())
}

fn needs_serializer(_ty: &Type) -> bool {
    true
}

fn is_top_level(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Bytes
            | Type::Optional { .. }
            | Type::Sequence { .. }
            | Type::Map { .. }
            | Type::Record { .. }
            | Type::Enum { .. }
    )
}

/// Records sorted so a struct is defined before anything that embeds it.
fn order_records(model: &Model) -> Result<Vec<&RecordModel>> {
    let by_name: BTreeMap<&str, &RecordModel> = model
        .records
        .iter()
        .map(|record| (record.name.as_str(), record))
        .collect();

    let mut ordered: Vec<&RecordModel> = Vec::new();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut visiting: BTreeSet<String> = BTreeSet::new();

    for record in &model.records {
        visit_record(record, &by_name, &mut ordered, &mut done, &mut visiting)?;
    }
    Ok(ordered)
}

fn visit_record<'a>(
    record: &'a RecordModel,
    by_name: &BTreeMap<&str, &'a RecordModel>,
    ordered: &mut Vec<&'a RecordModel>,
    done: &mut BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
) -> Result<()> {
    if done.contains(&record.name) {
        return Ok(());
    }
    if !visiting.insert(record.name.clone()) {
        return Err(Error::UnsupportedFeature {
            context: record.name.clone(),
            feature: "records that contain themselves".to_string(),
        });
    }
    for field in &record.fields {
        for dependency in embedded_records(&field.ty) {
            if let Some(next) = by_name.get(dependency.as_str()) {
                visit_record(next, by_name, ordered, done, visiting)?;
            }
        }
    }
    visiting.remove(&record.name);
    done.insert(record.name.clone());
    ordered.push(record);
    Ok(())
}

fn embedded_records(ty: &Type) -> Vec<String> {
    match ty {
        Type::Record { name, .. } => vec![pascal(name)],
        Type::Optional { inner_type } | Type::Sequence { inner_type } => {
            embedded_records(inner_type)
        }
        Type::Map { value_type, .. } => embedded_records(value_type),
        _ => Vec::new(),
    }
}

fn docs(out: &mut Source, text: Option<&str>) {
    let Some(text) = text else { return };
    for line in text.lines() {
        out.line(format!("/// {}", line.trim()));
    }
}
