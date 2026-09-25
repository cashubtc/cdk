//! Emits a koffi binding used by the Node tests and the benchmark.
//!
//! This is not the shipped React Native path, which is Nitro over JSI. It
//! exists so the same Rust build can be exercised, and benchmarked against the
//! pure TypeScript implementation, inside one Node process. It is generated
//! from the same model, so it cannot drift from the Rust API.

use uniffi_bindgen::interface::Type;

use crate::error::{Error, Result};
use crate::model::{CallableKind, CallableModel, Model};
use crate::naming::pascal;
use crate::writer::{banner, Source};

/// The koffi harness module.
pub fn harness(model: &Model, crate_name: &str) -> Result<String> {
    let mut out = Source::new();
    out.lines(&banner("//", crate_name));
    out.line("//");
    out.line("// Test harness only. The shipped React Native path is Nitro over JSI;");
    out.line("// this exists so Node can exercise and benchmark the same Rust build.");
    out.blank();
    out.line("import koffi from 'koffi'");
    out.blank();
    out.lines(RUNTIME);
    out.blank();

    emit_error_class(&mut out, model);
    emit_reader_writer_for(&mut out, model)?;

    out.line("/** Load the Rust library and return the generated API. */");
    out.line("export function load(libraryPath) {");
    out.indented(|out| {
        out.line("const lib = koffi.load(libraryPath)");
        out.line("const fn = {}");
        emit_declarations(out, model);
        out.blank();
        emit_buffer_helpers(out, model);
        out.blank();
        if let Err(error) = emit_api(out, model) {
            out.line(format!("throw new Error({error:?})"));
        }
    });
    out.line("}");

    Ok(out.finish())
}

const RUNTIME: &str = r#"const RUST_CALL_SUCCESS = 0
const RUST_CALL_ERROR = 1
const RUST_CALL_UNEXPECTED_ERROR = 2

const RustBufferType = koffi.struct('UniffiRustBuffer', {
  capacity: 'uint64',
  len: 'uint64',
  data: 'void *',
})
const ForeignBytesType = koffi.struct('UniffiForeignBytes', {
  len: 'int32',
  data: 'void *',
})
const RustCallStatusType = koffi.struct('UniffiRustCallStatus', {
  code: 'int8',
  errorBuf: RustBufferType,
})
const StatusOut = koffi.inout(koffi.pointer(RustCallStatusType))

/** Big-endian reader over the UniFFI buffer format. */
class Reader {
  constructor(bytes) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    this.bytes = bytes
    this.position = 0
  }
  readI8() { const v = this.view.getInt8(this.position); this.position += 1; return v }
  readU8() { const v = this.view.getUint8(this.position); this.position += 1; return v }
  readBool() { return this.readI8() !== 0 }
  readI16() { const v = this.view.getInt16(this.position); this.position += 2; return v }
  readU16() { const v = this.view.getUint16(this.position); this.position += 2; return v }
  readI32() { const v = this.view.getInt32(this.position); this.position += 4; return v }
  readU32() { const v = this.view.getUint32(this.position); this.position += 4; return v }
  readI64() { const v = this.view.getBigInt64(this.position); this.position += 8; return v }
  readU64() { const v = this.view.getBigUint64(this.position); this.position += 8; return v }
  readF32() { const v = this.view.getFloat32(this.position); this.position += 4; return v }
  readF64() { const v = this.view.getFloat64(this.position); this.position += 8; return v }
  readLength() {
    const length = this.readI32()
    if (length < 0) throw new Error('uniffi buffer declares a negative length')
    return length
  }
  readBytes() {
    const length = this.readLength()
    const slice = this.bytes.subarray(this.position, this.position + length)
    this.position += length
    return new Uint8Array(slice)
  }
  readString() {
    return new TextDecoder().decode(this.readBytes())
  }
}

/** Big-endian writer producing the UniFFI buffer format. */
class Writer {
  constructor() {
    this.chunks = []
    this.size = 0
  }
  push(bytes) { this.chunks.push(bytes); this.size += bytes.length }
  scalar(width, write) {
    const buffer = new Uint8Array(width)
    write(new DataView(buffer.buffer))
    this.push(buffer)
  }
  writeI8(v) { this.scalar(1, (d) => d.setInt8(0, v)) }
  writeU8(v) { this.scalar(1, (d) => d.setUint8(0, v)) }
  writeBool(v) { this.writeI8(v ? 1 : 0) }
  writeI16(v) { this.scalar(2, (d) => d.setInt16(0, v)) }
  writeU16(v) { this.scalar(2, (d) => d.setUint16(0, v)) }
  writeI32(v) { this.scalar(4, (d) => d.setInt32(0, v)) }
  writeU32(v) { this.scalar(4, (d) => d.setUint32(0, v)) }
  writeI64(v) { this.scalar(8, (d) => d.setBigInt64(0, BigInt(v))) }
  writeU64(v) { this.scalar(8, (d) => d.setBigUint64(0, BigInt(v))) }
  writeF32(v) { this.scalar(4, (d) => d.setFloat32(0, v)) }
  writeF64(v) { this.scalar(8, (d) => d.setFloat64(0, v)) }
  writeLength(n) { this.writeI32(n) }
  writeBytes(bytes) { this.writeLength(bytes.length); this.push(bytes) }
  writeString(text) { this.writeBytes(new TextEncoder().encode(text)) }
  finish() {
    const out = new Uint8Array(this.size)
    let offset = 0
    for (const chunk of this.chunks) {
      out.set(chunk, offset)
      offset += chunk.length
    }
    return out
  }
}

function newStatus() {
  return { code: 0, errorBuf: { capacity: 0, len: 0, data: null } }
}"#;

fn emit_error_class(out: &mut Source, model: &Model) {
    for error in &model.errors {
        out.line(format!(
            "/** The Rust `{}`, rebuilt from the structured payload. */",
            error.name
        ));
        out.line(format!("export class {} extends Error {{", error.name));
        out.indented(|out| {
            out.line("constructor(kind, message, fields) {");
            out.indented(|out| {
                out.line("super(message)");
                out.line(format!("this.name = '{}'", error.name));
                out.line("this.kind = kind");
                out.line("this.fields = fields");
            });
            out.line("}");
        });
        out.line("}");
        out.blank();
    }
}

fn emit_reader_writer_for(out: &mut Source, model: &Model) -> Result<()> {
    for (ser, (ty, _)) in &model.serialized {
        if matches!(ty, Type::Object { .. }) {
            continue;
        }
        out.line(format!("function read{ser}(reader) {{"));
        out.indented(|out| {
            out.lines(&read_body(model, ty, ser));
        });
        out.line("}");
        out.blank();

        out.line(format!("function write{ser}(writer, value) {{"));
        out.indented(|out| {
            out.lines(&write_body(model, ty));
        });
        out.line("}");
        out.blank();
    }
    Ok(())
}

fn read_body(model: &Model, ty: &Type, ser: &str) -> String {
    let mut out = Source::new();
    match ty {
        Type::Boolean => out.line("return reader.readBool()"),
        Type::Int8 => out.line("return reader.readI8()"),
        Type::UInt8 => out.line("return reader.readU8()"),
        Type::Int16 => out.line("return reader.readI16()"),
        Type::UInt16 => out.line("return reader.readU16()"),
        Type::Int32 => out.line("return reader.readI32()"),
        Type::UInt32 => out.line("return reader.readU32()"),
        Type::Int64 => out.line("return reader.readI64()"),
        Type::UInt64 => out.line("return reader.readU64()"),
        Type::Float32 => out.line("return reader.readF32()"),
        Type::Float64 => out.line("return reader.readF64()"),
        Type::String => out.line("return reader.readString()"),
        Type::Bytes => out.line("return reader.readBytes()"),
        Type::Optional { inner_type } => {
            let inner = child(inner_type);
            out.line("if (reader.readI8() === 0) return undefined");
            out.line(format!("return read{inner}(reader)"))
        }
        Type::Sequence { inner_type } => {
            let inner = child(inner_type);
            out.line("const count = reader.readLength()");
            out.line("const items = new Array(count)");
            out.line(format!(
                "for (let i = 0; i < count; i++) items[i] = read{inner}(reader)"
            ));
            out.line("return items")
        }
        Type::Map { value_type, .. } => {
            let inner = child(value_type);
            out.line("const count = reader.readLength()");
            out.line("const entries = {}");
            out.line("for (let i = 0; i < count; i++) {");
            out.indented(|out| {
                out.line("const key = reader.readString()");
                out.line(format!("entries[key] = read{inner}(reader)"));
            });
            out.line("}");
            out.line("return entries")
        }
        Type::Record { name, .. } => {
            let record = model.records.iter().find(|r| r.name == pascal(name));
            out.line("return {");
            out.indented(|out| {
                if let Some(record) = record {
                    for field in &record.fields {
                        out.line(format!(
                            "{}: read{}(reader),",
                            field.js_name,
                            child(&field.ty)
                        ));
                    }
                }
            });
            out.line("}")
        }
        Type::Enum { name, .. } => {
            let enum_ = model
                .enums
                .iter()
                .chain(model.errors.iter())
                .find(|e| e.name == pascal(name));
            out.line("const tag = reader.readI32()");
            if let Some(enum_) = enum_ {
                out.line("switch (tag) {");
                out.indented(|out| {
                    for (index, variant) in enum_.variants.iter().enumerate() {
                        out.line(format!("case {}: return '{}'", index + 1, variant.js_name));
                    }
                });
                out.line("}");
            }
            out.line(format!(
                "throw new Error(`unknown {ser} variant tag ${{tag}}`)"
            ))
        }
        _ => out.line("throw new Error('unsupported type')"),
    };
    out.finish()
}

fn write_body(model: &Model, ty: &Type) -> String {
    let mut out = Source::new();
    match ty {
        Type::Boolean => out.line("writer.writeBool(value)"),
        Type::Int8 => out.line("writer.writeI8(value)"),
        Type::UInt8 => out.line("writer.writeU8(value)"),
        Type::Int16 => out.line("writer.writeI16(value)"),
        Type::UInt16 => out.line("writer.writeU16(value)"),
        Type::Int32 => out.line("writer.writeI32(value)"),
        Type::UInt32 => out.line("writer.writeU32(value)"),
        Type::Int64 => out.line("writer.writeI64(value)"),
        Type::UInt64 => out.line("writer.writeU64(value)"),
        Type::Float32 => out.line("writer.writeF32(value)"),
        Type::Float64 => out.line("writer.writeF64(value)"),
        Type::String => out.line("writer.writeString(value)"),
        Type::Bytes => out.line("writer.writeBytes(value)"),
        Type::Optional { inner_type } => {
            let inner = child(inner_type);
            out.line("if (value === undefined || value === null) { writer.writeI8(0); return }");
            out.line("writer.writeI8(1)");
            out.line(format!("write{inner}(writer, value)"))
        }
        Type::Sequence { inner_type } => {
            let inner = child(inner_type);
            out.line("writer.writeLength(value.length)");
            out.line(format!(
                "for (const item of value) write{inner}(writer, item)"
            ))
        }
        Type::Map { value_type, .. } => {
            let inner = child(value_type);
            out.line("const keys = Object.keys(value)");
            out.line("writer.writeLength(keys.length)");
            out.line("for (const key of keys) {");
            out.indented(|out| {
                out.line("writer.writeString(key)");
                out.line(format!("write{inner}(writer, value[key])"));
            });
            out.line("}")
        }
        Type::Record { name, .. } => {
            let record = model.records.iter().find(|r| r.name == pascal(name));
            if let Some(record) = record {
                for field in &record.fields {
                    out.line(format!(
                        "write{}(writer, value.{})",
                        child(&field.ty),
                        field.js_name
                    ));
                }
            }
            out.line("")
        }
        Type::Enum { name, .. } => {
            let enum_ = model.enums.iter().find(|e| e.name == pascal(name));
            if let Some(enum_) = enum_ {
                out.line("switch (value) {");
                out.indented(|out| {
                    for (index, variant) in enum_.variants.iter().enumerate() {
                        out.line(format!(
                            "case '{}': writer.writeI32({}); return",
                            variant.js_name,
                            index + 1
                        ));
                    }
                });
                out.line("}");
            }
            out.line(format!(
                "throw new Error(`unknown {} value ${{value}}`)",
                pascal(name)
            ))
        }
        _ => out.line("throw new Error('unsupported type')"),
    };
    out.finish()
}

fn emit_declarations(out: &mut Source, model: &Model) {
    let mut declared = std::collections::BTreeSet::new();
    let all = model.root_methods.iter().chain(
        model
            .objects
            .iter()
            .flat_map(|object| object.methods.iter()),
    );
    for callable in all {
        if !declared.insert(callable.symbol.clone()) {
            continue;
        }
        let mut args: Vec<String> = Vec::new();
        if matches!(callable.kind, CallableKind::Method { .. }) {
            args.push("'uint64'".to_string());
        }
        for arg in &callable.args {
            args.push(koffi_type(&arg.ty));
        }
        if callable.has_status {
            args.push("StatusOut".to_string());
        }
        let returns = match &callable.returns {
            Some((ty, _)) => koffi_type(ty),
            None => "'void'".to_string(),
        };
        out.line(format!(
            "fn['{symbol}'] = lib.func('{symbol}', {returns}, [{}])",
            args.join(", "),
            symbol = callable.symbol
        ));
    }

    for object in &model.objects {
        out.line(format!(
            "fn['{0}'] = lib.func('{0}', 'void', ['uint64', StatusOut])",
            object.free_symbol
        ));
        out.line(format!(
            "fn['{0}'] = lib.func('{0}', 'uint64', ['uint64', StatusOut])",
            object.clone_symbol
        ));
    }
    out.line(format!(
        "fn['{0}'] = lib.func('{0}', RustBufferType, [ForeignBytesType, StatusOut])",
        model.rustbuffer_from_bytes
    ));
    out.line(format!(
        "fn['{0}'] = lib.func('{0}', 'void', [RustBufferType, StatusOut])",
        model.rustbuffer_free
    ));
    out.line(format!(
        "fn['{0}'] = lib.func('{0}', 'uint32', [])",
        model.contract_version_symbol
    ));
    for (symbol, _) in &model.checksums {
        out.line(format!(
            "fn['{symbol}'] = lib.func('{symbol}', 'uint16', [])"
        ));
    }
    out.blank();
    out.line(format!(
        "if (fn['{}']() !== {}) throw new Error('uniffi ABI mismatch: rebuild the bindings')",
        model.contract_version_symbol, model.contract_version
    ));
    for (symbol, expected) in &model.checksums {
        out.line(format!(
            "if (fn['{symbol}']() !== {expected}) throw new Error('uniffi checksum mismatch for {symbol}: rebuild the bindings')"
        ));
    }
}

fn emit_buffer_helpers(out: &mut Source, model: &Model) {
    out.lines(&format!(
        r#"function freeBuffer(buffer) {{
  if (buffer.data === null) return
  const status = newStatus()
  fn['{free}'](buffer, status)
}}

function consumeBuffer(buffer) {{
  const length = Number(buffer.len)
  const bytes = length > 0 ? new Uint8Array(koffi.view(buffer.data, length).slice(0)) : new Uint8Array(0)
  freeBuffer(buffer)
  return bytes
}}

function toBuffer(bytes) {{
  const status = newStatus()
  const buffer = fn['{from_bytes}']({{ len: bytes.length, data: bytes }}, status)
  if (status.code !== RUST_CALL_SUCCESS) throw new Error('uniffi could not allocate a buffer')
  return buffer
}}

function lower(write, value) {{
  const writer = new Writer()
  write(writer, value)
  return toBuffer(writer.finish())
}}

function lift(read, buffer) {{
  return read(new Reader(consumeBuffer(buffer)))
}}

function stringToBuffer(text) {{
  return toBuffer(new TextEncoder().encode(text))
}}

function bufferToString(buffer) {{
  return new TextDecoder().decode(consumeBuffer(buffer))
}}

function checkUnexpected(status) {{
  if (status.code === RUST_CALL_UNEXPECTED_ERROR) {{
    throw new Error('rust panicked: ' + bufferToString(status.errorBuf))
  }}
}}"#,
        free = model.rustbuffer_free,
        from_bytes = model.rustbuffer_from_bytes,
    ));

    for error in &model.errors {
        out.blank();
        out.line(format!("function throw{}(buffer) {{", error.name));
        out.indented(|out| {
            out.line("const reader = new Reader(consumeBuffer(buffer))");
            out.line("const tag = reader.readI32()");
            out.line("switch (tag) {");
            out.indented(|out| {
                for (index, variant) in error.variants.iter().enumerate() {
                    out.line(format!("case {}: {{", index + 1));
                    out.indented(|out| {
                        let mut names = Vec::new();
                        for field in &variant.fields {
                            out.line(format!(
                                "const {} = read{}(reader)",
                                field.js_name,
                                child(&field.ty)
                            ));
                            names.push(field.js_name.clone());
                        }
                        let fields = names
                            .iter()
                            .map(|name| format!("{name}: String({name})"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let message = if names.is_empty() {
                            format!("'{}'", variant.rust_name)
                        } else {
                            format!(
                                "`{}(${{[{}].join(', ')}})`",
                                variant.rust_name,
                                names
                                    .iter()
                                    .map(|name| format!("'{name}=' + {name}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        out.line(format!(
                            "throw new {}('{}', {message}, {{ {fields} }})",
                            error.name, variant.rust_name
                        ));
                    });
                    out.line("}");
                }
            });
            out.line("}");
            out.line(format!(
                "throw new {}('Unknown', 'unknown variant', {{}})",
                error.name
            ));
        });
        out.line("}");
    }
}

fn emit_api(out: &mut Source, model: &Model) -> Result<()> {
    for object in &model.objects {
        let mut body = Source::new();
        body.line("constructor(handle) { this.handle = handle }");
        body.blank();
        body.line("cloneHandle() {");
        body.indented(|body| {
            body.line(
                "if (this.handle === 0) throw new Error('use of a native object after close()')",
            );
            body.line("const status = newStatus()");
            body.line(format!(
                "const cloned = fn['{}'](this.handle, status)",
                object.clone_symbol
            ));
            body.line("checkUnexpected(status)");
            body.line("return cloned");
        });
        body.line("}");
        body.blank();
        body.line("close() {");
        body.indented(|body| {
            body.line("if (this.handle === 0) return");
            body.line("const status = newStatus()");
            body.line(format!("fn['{}'](this.handle, status)", object.free_symbol));
            body.line("this.handle = 0");
        });
        body.line("}");
        for method in &object.methods {
            body.blank();
            emit_callable(&mut body, method, true)?;
        }

        out.line(format!("class {} {{", object.name));
        out.indented(|out| {
            out.lines(&body.clone_text());
        });
        out.line("}");
        out.blank();
    }

    let mut api = Source::new();
    for method in &model.root_methods {
        emit_callable(&mut api, method, false)?;
    }
    out.line("return {");
    out.indented(|out| {
        out.lines(&api.clone_text());
    });
    out.line("}");
    Ok(())
}

fn emit_callable(out: &mut Source, callable: &CallableModel, is_method: bool) -> Result<()> {
    let params = callable
        .args
        .iter()
        .map(|arg| arg.js_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let head = if is_method {
        format!("{}({params}) {{", callable.js_name)
    } else {
        format!("{}: ({params}) => {{", callable.js_name)
    };
    out.line(head);

    let mut call_args = Vec::new();
    if is_method {
        call_args.push("this.cloneHandle()".to_string());
    }
    let mut body = Source::new();
    for arg in &callable.args {
        let lowered = format!("lowered_{}", arg.js_name);
        body.line(format!(
            "const {lowered} = {}",
            node_lower(&arg.ty, &arg.js_name, &arg.mapped.ser)?
        ));
        call_args.push(lowered);
    }
    if callable.has_status {
        body.line("const status = newStatus()");
        call_args.push("status".to_string());
    }
    let call = format!("fn['{}']({})", callable.symbol, call_args.join(", "));
    if callable.returns.is_some() {
        body.line(format!("const raw = {call}"));
    } else {
        body.line(call);
    }
    if callable.has_status {
        if let Some(error) = &callable.throws {
            body.line(format!(
                "if (status.code === RUST_CALL_ERROR) throw{error}(status.errorBuf)"
            ));
        }
        body.line("checkUnexpected(status)");
    }
    if let Some((ty, mapped)) = &callable.returns {
        body.line(format!("return {}", node_lift(ty, "raw", &mapped.ser)?));
    }

    out.indented(|out| {
        out.lines(&body.finish());
    });
    out.line(if is_method { "}" } else { "}," });
    Ok(())
}

fn node_lower(ty: &Type, value: &str, ser: &str) -> Result<String> {
    Ok(match ty {
        Type::Boolean => format!("({value} ? 1 : 0)"),
        Type::Int64 | Type::UInt64 => format!("BigInt({value})"),
        Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Float32
        | Type::Float64 => value.to_string(),
        Type::String => format!("stringToBuffer({value})"),
        Type::Bytes
        | Type::Enum { .. }
        | Type::Record { .. }
        | Type::Optional { .. }
        | Type::Sequence { .. }
        | Type::Map { .. } => format!("lower(write{ser}, {value})"),
        Type::Custom { builtin, .. } => node_lower(builtin, value, ser)?,
        other => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("{other:?}"),
                hint: " in the Node harness".to_string(),
            })
        }
    })
}

fn node_lift(ty: &Type, value: &str, ser: &str) -> Result<String> {
    Ok(match ty {
        Type::Boolean => format!("({value} !== 0)"),
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
        Type::String => format!("bufferToString({value})"),
        Type::Bytes
        | Type::Enum { .. }
        | Type::Record { .. }
        | Type::Optional { .. }
        | Type::Sequence { .. }
        | Type::Map { .. } => format!("lift(read{ser}, {value})"),
        Type::Object { name, .. } => format!("new {}({value})", pascal(name)),
        Type::Custom { builtin, .. } => node_lift(builtin, value, ser)?,
        other => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("{other:?}"),
                hint: " in the Node harness".to_string(),
            })
        }
    })
}

fn koffi_type(ty: &Type) -> String {
    match ty {
        Type::Boolean | Type::Int8 => "'int8'".to_string(),
        Type::UInt8 => "'uint8'".to_string(),
        Type::Int16 => "'int16'".to_string(),
        Type::UInt16 => "'uint16'".to_string(),
        Type::Int32 => "'int32'".to_string(),
        Type::UInt32 => "'uint32'".to_string(),
        Type::Int64 => "'int64'".to_string(),
        Type::UInt64 => "'uint64'".to_string(),
        Type::Float32 => "'float'".to_string(),
        Type::Float64 => "'double'".to_string(),
        Type::Object { .. } => "'uint64'".to_string(),
        Type::Custom { builtin, .. } => koffi_type(builtin),
        _ => "RustBufferType".to_string(),
    }
}

fn child(ty: &Type) -> String {
    crate::typemap::map(ty, "nested")
        .map(|mapped| mapped.ser)
        .unwrap_or_else(|_| "Unsupported".to_string())
}
