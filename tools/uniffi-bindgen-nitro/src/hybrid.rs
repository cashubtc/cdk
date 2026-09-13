//! Emits the Nitro `HybridObject` implementations.
//!
//! These are generated after nitrogen runs, so every override signature is
//! copied from the spec nitrogen just produced instead of being re-derived.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use uniffi_bindgen::interface::Type;

use crate::config::NitroConfig;
use crate::error::{Error, Result};
use crate::model::{CallableModel, Model, ObjectModel};
use crate::naming::pascal;
use crate::spec_parser::{self, SpecMethod};
use crate::typemap::map;
use crate::writer::{banner, Source};
use crate::GeneratedFile;

/// Generate one hybrid implementation per exported object plus the root module.
pub fn generate(
    model: &Model,
    config: &NitroConfig,
    crate_name: &str,
    nitrogen_dir: &Utf8Path,
) -> Result<Vec<GeneratedFile>> {
    let mut files = Vec::new();

    for object in &model.objects {
        let spec = read_spec(nitrogen_dir, &object.name)?;
        files.push(GeneratedFile {
            path: Utf8PathBuf::from(format!("Hybrid{}.hpp", object.name)),
            contents: object_header(model, crate_name, object, &spec)?,
        });
        files.push(GeneratedFile {
            path: Utf8PathBuf::from(format!("Hybrid{}.cpp", object.name)),
            contents: object_source(model, config, crate_name, object, &spec)?,
        });
    }

    let spec = read_spec(nitrogen_dir, &model.module)?;
    files.push(GeneratedFile {
        path: Utf8PathBuf::from(format!("Hybrid{}.hpp", model.module)),
        contents: root_header(model, crate_name, &spec)?,
    });
    files.push(GeneratedFile {
        path: Utf8PathBuf::from(format!("Hybrid{}.cpp", model.module)),
        contents: root_source(model, config, crate_name, &spec)?,
    });

    Ok(files)
}

fn read_spec(nitrogen_dir: &Utf8Path, name: &str) -> Result<BTreeMap<String, SpecMethod>> {
    let path = nitrogen_dir.join(format!("generated/shared/c++/Hybrid{name}Spec.hpp"));
    let source = fs::read_to_string(&path).map_err(|err| Error::SpecParse {
        path: path.to_string(),
        reason: format!("{err}; run nitrogen before generating the adapters"),
    })?;
    Ok(spec_parser::parse(path.as_str(), &source)?
        .into_iter()
        .map(|method| (method.name.clone(), method))
        .collect())
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

fn root_header(
    model: &Model,
    crate_name: &str,
    spec: &BTreeMap<String, SpecMethod>,
) -> Result<String> {
    let mut out = Source::new();
    header_preamble(&mut out, model, crate_name, &model.module);
    out.line("/// Root native module. Owns no state; every call goes straight to Rust.");
    out.line(format!(
        "class Hybrid{0} final : public Hybrid{0}Spec {{",
        model.module
    ));
    out.line("public:");
    out.indented(|out| {
        out.line(format!("Hybrid{}() : HybridObject(TAG) {{", model.module));
        out.indented(|out| {
            out.line("::{NS}::bridge::assertAbiCompatible();");
        });
        out.line("}");
        out.blank();
    });
    let mut body = Source::new();
    body.indented(|out| {
        for method in &model.root_methods {
            if let Some(declared) = spec.get(&method.js_name) {
                out.line(format!(
                    "{} {}({}) override;",
                    declared.return_type,
                    declared.name,
                    declared.param_list()
                ));
            }
        }
    });
    out.lines(&body.finish());
    out.line("};");
    out.blank();
    out.line(format!(
        "}} // namespace margelo::nitro::{}",
        model.cxx_namespace
    ));
    Ok(out.finish().replace("{NS}", &model.cxx_namespace))
}

fn object_header(
    model: &Model,
    crate_name: &str,
    object: &ObjectModel,
    spec: &BTreeMap<String, SpecMethod>,
) -> Result<String> {
    let mut out = Source::new();
    header_preamble(&mut out, model, crate_name, &object.name);
    out.line(format!(
        "/// Wraps the Rust `{}`; the handle dies with this object.",
        object.name
    ));
    out.line(format!(
        "class Hybrid{0} final : public Hybrid{0}Spec {{",
        object.name
    ));
    out.line("public:");
    out.indented(|out| {
        out.line(format!(
            "explicit Hybrid{0}(std::shared_ptr<::{{NS}}::bridge::{0}> inner)",
            object.name
        ));
        out.indented(|out| {
            out.line(": HybridObject(TAG), inner_(std::move(inner)) {}");
        });
        out.blank();
        for method in &object.methods {
            if let Some(declared) = spec.get(&method.js_name) {
                out.line(format!(
                    "{} {}({}) override;",
                    declared.return_type,
                    declared.name,
                    declared.param_list()
                ));
            }
        }
    });
    out.blank();
    out.line("private:");
    out.indented(|out| {
        out.line(format!(
            "std::shared_ptr<::{{NS}}::bridge::{}> inner_;",
            object.name
        ));
    });
    out.line("};");
    out.blank();
    out.line(format!(
        "}} // namespace margelo::nitro::{}",
        model.cxx_namespace
    ));
    Ok(out.finish().replace("{NS}", &model.cxx_namespace))
}

fn header_preamble(out: &mut Source, model: &Model, crate_name: &str, spec_name: &str) {
    out.lines(&banner("//", crate_name));
    out.blank();
    out.line("#pragma once");
    out.blank();
    out.line(format!("#include \"Hybrid{spec_name}Spec.hpp\""));
    for object in &model.objects {
        if object.name != spec_name {
            out.line(format!("#include \"Hybrid{}Spec.hpp\"", object.name));
        }
    }
    out.line(format!("#include \"{}Bridge.hpp\"", model.module));
    out.blank();
    out.line("#include <memory>");
    out.line("#include <optional>");
    out.line("#include <string>");
    out.line("#include <vector>");
    out.blank();
    out.line(format!(
        "namespace margelo::nitro::{} {{",
        model.cxx_namespace
    ));
    out.blank();
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

fn root_source(
    model: &Model,
    config: &NitroConfig,
    crate_name: &str,
    spec: &BTreeMap<String, SpecMethod>,
) -> Result<String> {
    let mut out = Source::new();
    source_preamble(&mut out, model, crate_name, &model.module);
    emit_converters(&mut out, model, &referenced(model, &model.root_methods))?;
    for method in &model.root_methods {
        emit_method(&mut out, model, config, method, spec, &model.module, None)?;
    }
    out.line(format!(
        "}} // namespace margelo::nitro::{}",
        model.cxx_namespace
    ));
    Ok(out.finish().replace("{NS}", &model.cxx_namespace))
}

fn object_source(
    model: &Model,
    config: &NitroConfig,
    crate_name: &str,
    object: &ObjectModel,
    spec: &BTreeMap<String, SpecMethod>,
) -> Result<String> {
    let mut out = Source::new();
    source_preamble(&mut out, model, crate_name, &object.name);
    emit_converters(&mut out, model, &referenced(model, &object.methods))?;
    for method in &object.methods {
        emit_method(
            &mut out,
            model,
            config,
            method,
            spec,
            &object.name,
            Some(object),
        )?;
    }
    out.line(format!(
        "}} // namespace margelo::nitro::{}",
        model.cxx_namespace
    ));
    Ok(out.finish().replace("{NS}", &model.cxx_namespace))
}

fn source_preamble(out: &mut Source, model: &Model, crate_name: &str, spec_name: &str) {
    out.lines(&banner("//", crate_name));
    out.blank();
    out.line(format!("#include \"Hybrid{spec_name}.hpp\""));
    for object in &model.objects {
        if object.name != spec_name {
            out.line(format!("#include \"Hybrid{}.hpp\"", object.name));
        }
    }
    out.blank();
    out.line("#include <NitroModules/ArrayBuffer.hpp>");
    out.line("#include <NitroModules/Promise.hpp>");
    out.blank();
    out.line("#include <utility>");
    out.blank();
    out.line(format!(
        "namespace margelo::nitro::{} {{",
        model.cxx_namespace
    ));
    out.blank();
}

/// Every record and enum whose Nitro type is visible in this translation unit.
///
/// Nitrogen only includes the types a spec actually mentions, so a converter
/// for anything else would not compile.
fn referenced(model: &Model, methods: &[CallableModel]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for method in methods {
        for arg in &method.args {
            walk_type(model, &arg.ty, &mut names);
        }
        if let Some((ty, _)) = &method.returns {
            walk_type(model, ty, &mut names);
        }
    }
    names
}

fn walk_type(model: &Model, ty: &Type, names: &mut BTreeSet<String>) {
    match ty {
        Type::Optional { inner_type } | Type::Sequence { inner_type } => {
            walk_type(model, inner_type, names);
        }
        Type::Map { value_type, .. } => walk_type(model, value_type, names),
        Type::Custom { builtin, .. } => walk_type(model, builtin, names),
        Type::Record { name, .. } => {
            let name = pascal(name);
            if !names.insert(name.clone()) {
                return;
            }
            if let Some(record) = model.records.iter().find(|record| record.name == name) {
                for field in &record.fields {
                    walk_type(model, &field.ty, names);
                }
            }
        }
        Type::Enum { name, .. } => {
            names.insert(pascal(name));
        }
        _ => {}
    }
}

/// Record and enum converters, emitted once per translation unit.
fn emit_converters(out: &mut Source, model: &Model, wanted: &BTreeSet<String>) -> Result<()> {
    out.line("namespace {");
    out.blank();

    for enum_ in model
        .enums
        .iter()
        .filter(|item| wanted.contains(&item.name))
    {
        let bridge = format!("::{{NS}}::bridge::{}", enum_.name);
        out.line(format!("{bridge} toBridge{0}({0} value);", enum_.name));
        out.line(format!("{0} fromBridge{0}({bridge} value);", enum_.name));
    }
    out.blank();

    for record in model
        .records
        .iter()
        .filter(|item| wanted.contains(&item.name))
    {
        let bridge = format!("::{{NS}}::bridge::{}", record.name);
        let assignments = record
            .fields
            .iter()
            .map(|field| {
                let expr = to_bridge(&field.ty, &format!("value.{}", field.js_name))
                    .map_err(|err| in_record(&record.name, &field.js_name, err))?;
                Ok(format!("out.{} = {expr};", field.js_name))
            })
            .collect::<Result<Vec<_>>>()?;

        out.line(format!(
            "{bridge} toBridge{0}(const {0}& value) {{",
            record.name
        ));
        out.indented(|out| {
            out.line(format!("{bridge} out;"));
            for assignment in &assignments {
                out.line(assignment);
            }
            out.line("return out;");
        });
        out.line("}");
        out.blank();

        let args = record
            .fields
            .iter()
            .map(|field| {
                from_bridge(&field.ty, &format!("value.{}", field.js_name), false)
                    .map_err(|err| in_record(&record.name, &field.js_name, err))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        out.line(format!(
            "{0} fromBridge{0}(const {bridge}& value) {{",
            record.name
        ));
        out.indented(|out| {
            out.line(format!("return {}({args});", record.name));
        });
        out.line("}");
        out.blank();
    }

    for enum_ in model
        .enums
        .iter()
        .filter(|item| wanted.contains(&item.name))
    {
        let bridge = format!("::{{NS}}::bridge::{}", enum_.name);
        out.line(format!("{bridge} toBridge{0}({0} value) {{", enum_.name));
        out.indented(|out| {
            out.line("switch (value) {");
            out.indented(|out| {
                for variant in &enum_.variants {
                    out.line(format!(
                        "case {}::{}: return {bridge}::{};",
                        enum_.name,
                        cpp_union_member(&variant.js_name),
                        variant.rust_name
                    ));
                }
            });
            out.line("}");
            out.line(format!(
                "throw std::runtime_error(\"unknown {} value\");",
                enum_.name
            ));
        });
        out.line("}");
        out.blank();

        out.line(format!("{0} fromBridge{0}({bridge} value) {{", enum_.name));
        out.indented(|out| {
            out.line("switch (value) {");
            out.indented(|out| {
                for variant in &enum_.variants {
                    out.line(format!(
                        "case {bridge}::{}: return {}::{};",
                        variant.rust_name,
                        enum_.name,
                        cpp_union_member(&variant.js_name)
                    ));
                }
            });
            out.line("}");
            out.line(format!(
                "throw std::runtime_error(\"unknown {} value\");",
                enum_.name
            ));
        });
        out.line("}");
        out.blank();
    }

    out.line("} // namespace");
    out.blank();
    Ok(())
}

fn emit_method(
    out: &mut Source,
    model: &Model,
    config: &NitroConfig,
    method: &CallableModel,
    spec: &BTreeMap<String, SpecMethod>,
    class: &str,
    owner: Option<&ObjectModel>,
) -> Result<()> {
    let Some(declared) = spec.get(&method.js_name) else {
        return Err(Error::SpecParse {
            path: format!("Hybrid{class}Spec.hpp"),
            reason: format!(
                "nitrogen produced no `{}` method; regenerate the .nitro.ts spec first",
                method.js_name
            ),
        });
    };
    if declared.params.len() != method.args.len() {
        return Err(Error::SpecParse {
            path: format!("Hybrid{class}Spec.hpp"),
            reason: format!(
                "`{}` takes {} parameters in the spec but {} in the Rust export",
                method.js_name,
                declared.params.len(),
                method.args.len()
            ),
        });
    }

    let is_async = config.is_async(&method.rust_name);
    out.line(format!(
        "{} Hybrid{class}::{}({}) {{",
        declared.return_type,
        declared.name,
        declared.param_list()
    ));

    let mut body = Source::new();
    build_call_body(&mut body, model, method, declared, owner, is_async)?;
    out.indented(|out| {
        out.lines(&body.finish());
    });
    out.line("}");
    out.blank();
    Ok(())
}

fn build_call_body(
    out: &mut Source,
    model: &Model,
    method: &CallableModel,
    declared: &SpecMethod,
    owner: Option<&ObjectModel>,
    is_async: bool,
) -> Result<()> {
    let mut call_args = Vec::new();
    for (arg, param) in method.args.iter().zip(declared.params.iter()) {
        let name = format!("bridge_{}", param.name);
        out.line(format!(
            "auto {name} = {};",
            to_bridge(&arg.ty, &param.name)?
        ));
        call_args.push(name);
    }

    let receiver = match owner {
        Some(_) => "inner_->".to_string(),
        None => "::{NS}::bridge::".to_string(),
    };
    let call = format!("{receiver}{}({})", method.js_name, call_args.join(", "));

    let returns_value = method.returns.is_some();
    if is_async {
        let inner_return = declared
            .return_type
            .trim_start_matches("std::shared_ptr<Promise<")
            .trim_end_matches(">>")
            .to_string();
        let captures = call_args
            .iter()
            .map(|name| format!("{name} = std::move({name})"))
            .collect::<Vec<_>>()
            .join(", ");
        let capture_self = match owner {
            Some(_) => {
                if captures.is_empty() {
                    "inner = inner_".to_string()
                } else {
                    format!("inner = inner_, {captures}")
                }
            }
            None => captures,
        };
        let call = match owner {
            Some(_) => format!("inner->{}({})", method.js_name, call_args.join(", ")),
            None => call.clone(),
        };
        out.line(format!(
            "return Promise<{inner_return}>::async([{capture_self}]() {{"
        ));
        out.indented(|out| {
            if returns_value {
                out.line(format!("auto result = {call};"));
                match emit_return(model, method, true) {
                    Ok(expr) => out.line(format!("return {expr};")),
                    Err(_) => out.line("return;"),
                };
            } else {
                out.line(format!("{call};"));
            }
        });
        out.line("});");
        return Ok(());
    }

    if returns_value {
        out.line(format!("auto result = {call};"));
        out.line(format!("return {};", emit_return(model, method, true)?));
    } else {
        out.line(format!("{call};"));
    }
    Ok(())
}

fn emit_return(model: &Model, method: &CallableModel, owned: bool) -> Result<String> {
    let Some((ty, _)) = &method.returns else {
        return Ok(String::new());
    };
    let _ = model;
    from_bridge(ty, "result", owned)
}

// ---------------------------------------------------------------------------
// Conversions between the Nitro and bridge representations
// ---------------------------------------------------------------------------

/// C++ expression converting a Nitro value into the bridge representation.
pub fn to_bridge(ty: &Type, value: &str) -> Result<String> {
    to_bridge_at(ty, value, 0)
}

/// `depth` keeps the temporaries of nested lambdas from shadowing each other.
fn to_bridge_at(ty: &Type, value: &str, depth: usize) -> Result<String> {
    Ok(match ty {
        Type::Boolean | Type::Float64 | Type::Int64 | Type::UInt64 | Type::String => {
            value.to_string()
        }
        Type::Int8 => format!("static_cast<int8_t>({value})"),
        Type::UInt8 => format!("static_cast<uint8_t>({value})"),
        Type::Int16 => format!("static_cast<int16_t>({value})"),
        Type::UInt16 => format!("static_cast<uint16_t>({value})"),
        Type::Int32 => format!("static_cast<int32_t>({value})"),
        Type::UInt32 => format!("static_cast<uint32_t>({value})"),
        Type::Float32 => format!("static_cast<float>({value})"),
        Type::Bytes => format!(
            "([&]{{ const auto& src{depth} = ({value}); return std::vector<uint8_t>(src{depth}->data(), src{depth}->data() + src{depth}->size()); }}())"
        ),
        Type::Optional { inner_type } => {
            let inner = map(inner_type, "optional")?;
            format!(
                "([&]{{ const auto& src{depth} = ({value}); std::optional<{}> out{depth}; if (src{depth}.has_value()) {{ out{depth} = {}; }} return out{depth}; }}())",
                inner.bridge.replace("bridge::", "::{NS}::bridge::"),
                to_bridge_at(inner_type, &format!("(*src{depth})"), depth + 1)?
            )
        }
        Type::Sequence { inner_type } => {
            let inner = map(inner_type, "sequence")?;
            format!(
                "([&]{{ const auto& src{depth} = ({value}); std::vector<{}> out{depth}; out{depth}.reserve(src{depth}.size()); for (const auto& item{depth} : src{depth}) {{ out{depth}.push_back({}); }} return out{depth}; }}())",
                inner.bridge.replace("bridge::", "::{NS}::bridge::"),
                to_bridge_at(inner_type, &format!("item{depth}"), depth + 1)?
            )
        }
        Type::Map { value_type, .. } => {
            let inner = map(value_type, "map")?;
            format!(
                "([&]{{ const auto& src{depth} = ({value}); std::unordered_map<std::string, {}> out{depth}; out{depth}.reserve(src{depth}.size()); for (const auto& entry{depth} : src{depth}) {{ out{depth}.emplace(entry{depth}.first, {}); }} return out{depth}; }}())",
                inner.bridge.replace("bridge::", "::{NS}::bridge::"),
                to_bridge_at(value_type, &format!("entry{depth}.second"), depth + 1)?
            )
        }
        Type::Record { name, .. } => format!("toBridge{}({value})", pascal(name)),
        Type::Enum { name, .. } => format!("toBridge{}({value})", pascal(name)),
        Type::Object { name, .. } => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("`{}` as an argument", pascal(name)),
                hint: "; native objects can only be returned, not passed back in".to_string(),
            })
        }
        Type::Custom { builtin, .. } => to_bridge_at(builtin, value, depth)?,
        other => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("{other:?}"),
                hint: String::new(),
            })
        }
    })
}

/// C++ expression converting a bridge value into the Nitro representation.
///
/// `owned` says the expression is a local the generator may move from.
pub fn from_bridge(ty: &Type, value: &str, owned: bool) -> Result<String> {
    from_bridge_at(ty, value, owned, 0)
}

fn from_bridge_at(ty: &Type, value: &str, owned: bool, depth: usize) -> Result<String> {
    Ok(match ty {
        Type::Boolean | Type::Float64 | Type::Int64 | Type::UInt64 => value.to_string(),
        Type::String => {
            if owned {
                format!("std::move({value})")
            } else {
                value.to_string()
            }
        }
        Type::Int8
        | Type::UInt8
        | Type::Int16
        | Type::UInt16
        | Type::Int32
        | Type::UInt32
        | Type::Float32 => format!("static_cast<double>({value})"),
        Type::Bytes => {
            if owned {
                format!("ArrayBuffer::move(std::move({value}))")
            } else {
                format!("ArrayBuffer::copy({value})")
            }
        }
        Type::Optional { inner_type } => {
            let inner = map(inner_type, "optional")?;
            format!(
                "([&]{{ const auto& src{depth} = ({value}); std::optional<{}> out{depth}; if (src{depth}.has_value()) {{ out{depth} = {}; }} return out{depth}; }}())",
                inner.nitro,
                from_bridge_at(inner_type, &format!("(*src{depth})"), false, depth + 1)?
            )
        }
        Type::Sequence { inner_type } => {
            let inner = map(inner_type, "sequence")?;
            format!(
                "([&]{{ const auto& src{depth} = ({value}); std::vector<{}> out{depth}; out{depth}.reserve(src{depth}.size()); for (const auto& item{depth} : src{depth}) {{ out{depth}.push_back({}); }} return out{depth}; }}())",
                inner.nitro,
                from_bridge_at(inner_type, &format!("item{depth}"), false, depth + 1)?
            )
        }
        Type::Map { value_type, .. } => {
            let inner = map(value_type, "map")?;
            format!(
                "([&]{{ const auto& src{depth} = ({value}); std::unordered_map<std::string, {}> out{depth}; out{depth}.reserve(src{depth}.size()); for (const auto& entry{depth} : src{depth}) {{ out{depth}.emplace(entry{depth}.first, {}); }} return out{depth}; }}())",
                inner.nitro,
                from_bridge_at(value_type, &format!("entry{depth}.second"), false, depth + 1)?
            )
        }
        Type::Record { name, .. } => format!("fromBridge{}({value})", pascal(name)),
        Type::Enum { name, .. } => format!("fromBridge{}({value})", pascal(name)),
        Type::Object { name, .. } => format!(
            "std::make_shared<Hybrid{}>(std::move({value}))",
            pascal(name)
        ),
        Type::Custom { builtin, .. } => from_bridge_at(builtin, value, owned, depth)?,
        other => {
            return Err(Error::UnsupportedType {
                context: value.to_string(),
                type_name: format!("{other:?}"),
                hint: String::new(),
            })
        }
    })
}

/// Re-points a field conversion failure at the record that declares it.
///
/// `to_bridge` and `from_bridge` only know the C++ expression they were
/// building, so on its own the error cannot say which record to go fix.
fn in_record(record: &str, field: &str, err: Error) -> Error {
    match err {
        Error::UnsupportedType {
            type_name, hint, ..
        } => Error::UnsupportedType {
            context: format!("record `{record}` field `{field}`"),
            type_name,
            hint,
        },
        other => other,
    }
}

/// Nitrogen's C++ member name for a TypeScript string-union literal.
fn cpp_union_member(literal: &str) -> String {
    literal
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FieldModel, RecordModel};

    fn model_with(record: RecordModel) -> Model {
        Model {
            module: "CashuCrypto".to_string(),
            cxx_namespace: "cashucrypto".to_string(),
            cdylib: "cashu_ffi".to_string(),
            records: vec![record],
            enums: Vec::new(),
            errors: Vec::new(),
            objects: Vec::new(),
            root_methods: Vec::new(),
            serialized: BTreeMap::new(),
            rustbuffer_alloc: String::new(),
            rustbuffer_free: String::new(),
            rustbuffer_from_bytes: String::new(),
            contract_version_symbol: String::new(),
            contract_version: 0,
            checksums: Vec::new(),
        }
    }

    fn field(js_name: &str, ty: Type) -> FieldModel {
        let mapped = map(&ty, js_name).expect("the type map accepts this field");
        FieldModel {
            rust_name: js_name.to_string(),
            js_name: js_name.to_string(),
            ty,
            mapped,
        }
    }

    fn converters_for(record: RecordModel) -> Result<String> {
        let wanted = BTreeSet::from([record.name.clone()]);
        let mut out = Source::default();
        emit_converters(&mut out, &model_with(record), &wanted)?;
        Ok(out.finish())
    }

    #[test]
    fn a_record_of_plain_fields_converts_in_both_directions() {
        let source = converters_for(RecordModel {
            name: "BlindPair".to_string(),
            fields: vec![field("blindedSecret", Type::String)],
            docs: None,
        })
        .expect("a string field is representable");

        assert!(source.contains("out.blindedSecret = value.blindedSecret;"));
        assert!(source.contains("return BlindPair(value.blindedSecret);"));
    }

    /// An object nested in a record used to emit `// unsupported field`, leaving
    /// a null `shared_ptr` for the serializer to call `cloneHandle()` on.
    #[test]
    fn a_record_carrying_an_object_is_refused() {
        let err = converters_for(RecordModel {
            name: "Wrapper".to_string(),
            fields: vec![field(
                "factory",
                Type::Object {
                    name: "DeterministicOutputFactory".to_string(),
                    module_path: String::new(),
                    imp: uniffi_bindgen::interface::ObjectImpl::Struct,
                },
            )],
            docs: None,
        })
        .expect_err("an object cannot be passed back in");

        let message = err.to_string();
        assert!(
            message.contains("record `Wrapper` field `factory`"),
            "{message}"
        );
        assert!(message.contains("DeterministicOutputFactory"), "{message}");
    }
}
