//! Emits the Nitro TypeScript spec and the typed error classes.

use uniffi_bindgen::interface::Type;

use crate::config::NitroConfig;
use crate::model::{CallableModel, EnumModel, Model, RecordModel};
use crate::writer::{banner, Source};

const PLATFORMS: &str = "HybridObject<{ ios: 'c++'; android: 'c++' }>";

/// The `.nitro.ts` spec nitrogen consumes.
pub fn spec(model: &Model, config: &NitroConfig, crate_name: &str) -> String {
    let mut out = Source::new();
    out.lines(&banner("//", crate_name));
    out.blank();
    out.line("import type { HybridObject, Int64, UInt64 } from 'react-native-nitro-modules'");
    out.blank();

    for enum_ in &model.enums {
        emit_enum(&mut out, enum_);
    }
    for record in &model.records {
        emit_record(&mut out, record);
    }
    for object in &model.objects {
        docs(&mut out, object.docs.as_deref());
        out.line(format!(
            "export interface {} extends {PLATFORMS} {{",
            object.name
        ));
        out.indented(|out| {
            for method in &object.methods {
                emit_signature(out, method, config);
            }
        });
        out.line("}");
        out.blank();
    }

    docs(
        &mut out,
        Some("Root native module. Every free function of the Rust crate hangs off it."),
    );
    out.line(format!(
        "export interface {} extends {PLATFORMS} {{",
        model.module
    ));
    out.indented(|out| {
        for method in &model.root_methods {
            emit_signature(out, method, config);
        }
    });
    out.line("}");

    out.finish()
}

/// Typed error classes plus the parser that rebuilds them from a native throw.
///
/// Nitro surfaces a C++ exception as a plain JavaScript `Error` carrying only
/// its message, so the adapter encodes the structured payload as JSON and this
/// module turns it back into a typed class.
pub fn errors(model: &Model, crate_name: &str) -> String {
    let mut out = Source::new();
    out.lines(&banner("//", crate_name));
    out.blank();

    let mut names = Vec::new();
    for error in &model.errors {
        out.line(format!(
            "/** Discriminant of {}, matching the Rust variant names. */",
            error.name
        ));
        out.line(format!("export type {}Kind =", error.name));
        out.indented(|out| {
            for (index, variant) in error.variants.iter().enumerate() {
                let sep = if index + 1 == error.variants.len() {
                    ";"
                } else {
                    ""
                };
                out.line(format!("| '{}'{sep}", variant.rust_name));
            }
        });
        out.blank();

        out.line(format!(
            "/** A `{}` raised by the native module. */",
            error.name
        ));
        out.line(format!("export class {} extends Error {{", error.name));
        out.indented(|out| {
            out.line(format!("readonly kind: {}Kind", error.name));
            out.line("readonly fields: Readonly<Record<string, unknown>>");
            out.blank();
            out.line(format!(
                "constructor(kind: {}Kind, message: string, fields: Record<string, unknown>) {{",
                error.name
            ));
            out.indented(|out| {
                out.line("super(message)");
                out.line(format!("this.name = '{}'", error.name));
                out.line("this.kind = kind");
                out.line("this.fields = Object.freeze(fields)");
            });
            out.line("}");
            for variant in &error.variants {
                out.blank();
                out.line(format!(
                    "/** Narrow to the `{}` variant. */",
                    variant.rust_name
                ));
                let fields = variant
                    .fields
                    .iter()
                    .map(|field| format!("{}: {}", field.js_name, json_field_ts(&field.ty)))
                    .collect::<Vec<_>>()
                    .join("; ");
                out.line(format!(
                    "is{}(): this is {} & {{ fields: {{ {fields} }} }} {{",
                    crate::naming::pascal(&variant.rust_name),
                    error.name
                ));
                out.indented(|out| {
                    out.line(format!("return this.kind === '{}'", variant.rust_name));
                });
                out.line("}");
            }
        });
        out.line("}");
        out.blank();
        names.push(error.name.clone());
    }

    out.line("/** Marker the native adapter prefixes structured errors with. */");
    out.line("const NATIVE_ERROR_PREFIX = 'uniffi-nitro-error:'");
    out.blank();
    out.line("/**");
    out.line(" * Rebuild a typed error from whatever the native module threw.");
    out.line(" *");
    out.line(" * Anything that is not a structured native error is returned unchanged, so a");
    out.line(" * genuine JavaScript failure is never disguised as a protocol error.");
    out.line(" */");
    out.line("export function toNativeError(thrown: unknown): unknown {");
    out.indented(|out| {
        out.line("const message = thrown instanceof Error ? thrown.message : String(thrown)");
        out.line("const start = message.indexOf(NATIVE_ERROR_PREFIX)");
        out.line("if (start < 0) return thrown");
        out.line("let payload: { type?: string; kind?: string; message?: string; fields?: Record<string, unknown> }");
        out.line("try {");
        out.indented(|out| {
            out.line("payload = JSON.parse(message.slice(start + NATIVE_ERROR_PREFIX.length))");
        });
        out.line("} catch {");
        out.indented(|out| { out.line("return thrown"); });
        out.line("}");
        out.line("switch (payload.type) {");
        out.indented(|out| {
            for name in &names {
                out.line(format!("case '{name}':"));
                out.indented(|out| {
                    out.line(format!(
                        "return new {name}(payload.kind as {name}Kind, payload.message ?? message, payload.fields ?? {{}})"
                    ));
                });
            }
            out.line("default:");
            out.indented(|out| { out.line("return thrown"); });
        });
        out.line("}");
    });
    out.line("}");
    out.blank();
    out.line("/** Run `call`, converting any structured native error it throws. */");
    out.line("export function withNativeErrors<T>(call: () => T): T {");
    out.indented(|out| {
        out.line("try {");
        out.indented(|out| {
            out.line("return call()");
        });
        out.line("} catch (thrown) {");
        out.indented(|out| {
            out.line("throw toNativeError(thrown)");
        });
        out.line("}");
    });
    out.line("}");

    out.finish()
}

/// How an error field looks after the round trip through the JSON payload.
///
/// 64-bit fields arrive as decimal strings and byte fields as hex, because JSON
/// has no way to carry either without loss.
fn json_field_ts(ty: &Type) -> String {
    match ty {
        Type::Int64 | Type::UInt64 | Type::Bytes | Type::Enum { .. } => "string".to_string(),
        Type::Boolean => "boolean".to_string(),
        Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Float32
        | Type::Float64 => "number".to_string(),
        Type::String => "string".to_string(),
        Type::Optional { inner_type } => format!("{} | null", json_field_ts(inner_type)),
        Type::Sequence { inner_type } => format!("{}[]", json_field_ts(inner_type)),
        Type::Custom { builtin, .. } => json_field_ts(builtin),
        _ => "unknown".to_string(),
    }
}

fn emit_enum(out: &mut Source, enum_: &EnumModel) {
    docs(out, enum_.docs.as_deref());
    let variants = enum_
        .variants
        .iter()
        .map(|variant| format!("'{}'", variant.js_name))
        .collect::<Vec<_>>()
        .join(" | ");
    out.line(format!("export type {} = {variants}", enum_.name));
    out.blank();
}

fn emit_record(out: &mut Source, record: &RecordModel) {
    docs(out, record.docs.as_deref());
    out.line(format!("export interface {} {{", record.name));
    out.indented(|out| {
        for field in &record.fields {
            let optional = matches!(field.ty, Type::Optional { .. });
            let (name, ty) = if optional {
                let inner = field
                    .mapped
                    .ts
                    .strip_suffix(" | undefined")
                    .unwrap_or(&field.mapped.ts);
                (format!("{}?", field.js_name), inner.to_string())
            } else {
                (field.js_name.clone(), field.mapped.ts.clone())
            };
            out.line(format!("{name}: {ty}"));
        }
    });
    out.line("}");
    out.blank();
}

fn emit_signature(out: &mut Source, method: &CallableModel, config: &NitroConfig) {
    docs(out, method.docs.as_deref());
    let last_required = method
        .args
        .iter()
        .rposition(|arg| !matches!(arg.ty, Type::Optional { .. }));

    let params = method
        .args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            let trailing_optional = matches!(arg.ty, Type::Optional { .. })
                && last_required.is_none_or(|last| index > last);
            if trailing_optional {
                let inner = arg
                    .mapped
                    .ts
                    .strip_suffix(" | undefined")
                    .unwrap_or(&arg.mapped.ts);
                format!("{}?: {inner}", arg.js_name)
            } else {
                format!("{}: {}", arg.js_name, arg.mapped.ts)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let returns = match &method.returns {
        Some((_, mapped)) => mapped.ts.clone(),
        None => "void".to_string(),
    };
    let returns = if config.is_async(&method.rust_name) {
        format!("Promise<{returns}>")
    } else {
        returns
    };

    out.line(format!("{}({params}): {returns}", method.js_name));
}

fn docs(out: &mut Source, text: Option<&str>) {
    let Some(text) = text else { return };
    let trimmed: Vec<&str> = text.lines().map(str::trim).collect();
    if trimmed.is_empty() {
        return;
    }
    out.line("/**");
    for line in trimmed {
        if line.is_empty() {
            out.line(" *");
        } else {
            out.line(format!(" * {line}"));
        }
    }
    out.line(" */");
}
