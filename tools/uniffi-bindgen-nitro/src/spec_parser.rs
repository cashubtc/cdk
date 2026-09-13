//! Reads the pure-virtual signatures out of a nitrogen-generated spec header.
//!
//! Copying nitrogen's C++ signature rules here would mean two places to keep in
//! step, so the override signatures are taken from the spec it just produced.

use crate::error::{Error, Result};

/// One `virtual ... = 0;` declaration.
#[derive(Debug, Clone)]
pub struct SpecMethod {
    /// Return type exactly as nitrogen spelled it.
    pub return_type: String,
    /// Method name.
    pub name: String,
    /// Parameters exactly as nitrogen spelled them, with their names.
    pub params: Vec<SpecParam>,
}

impl SpecMethod {
    /// The parameter list as written in the spec.
    pub fn param_list(&self) -> String {
        self.params
            .iter()
            .map(|param| format!("{} {}", param.ty, param.name))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One parameter of a spec method.
#[derive(Debug, Clone)]
pub struct SpecParam {
    /// Declared type, including any `const` and `&`.
    pub ty: String,
    /// Parameter name.
    pub name: String,
}

/// Parse every pure-virtual method of a spec header.
pub fn parse(path: &str, source: &str) -> Result<Vec<SpecMethod>> {
    let mut methods = Vec::new();
    let mut rest = source;

    while let Some(start) = rest.find("virtual ") {
        let after = &rest[start + "virtual ".len()..];
        let Some(end) = after.find("= 0;") else {
            break;
        };
        // A `virtual` that terminates before `= 0;` is a destructor or a
        // defaulted member, not something to override.
        if let Some(semicolon) = after.find(';') {
            if semicolon < end {
                rest = &after[semicolon + 1..];
                continue;
            }
        }

        let declaration = after[..end].trim();
        rest = &after[end + "= 0;".len()..];

        if declaration.is_empty() {
            continue;
        }
        methods.push(parse_declaration(path, declaration)?);
    }

    Ok(methods)
}

fn parse_declaration(path: &str, declaration: &str) -> Result<SpecMethod> {
    let open = declaration.find('(').ok_or_else(|| Error::SpecParse {
        path: path.to_string(),
        reason: format!("no parameter list in `{declaration}`"),
    })?;
    let close = declaration.rfind(')').ok_or_else(|| Error::SpecParse {
        path: path.to_string(),
        reason: format!("unterminated parameter list in `{declaration}`"),
    })?;

    let head = declaration[..open].trim();
    let split = head.rfind(|c: char| c.is_whitespace() || c == '*' || c == '&');
    let (return_type, name) = match split {
        Some(index) => (head[..=index].trim().to_string(), head[index + 1..].trim()),
        None => {
            return Err(Error::SpecParse {
                path: path.to_string(),
                reason: format!("could not split return type and name in `{declaration}`"),
            })
        }
    };

    let params = split_params(&declaration[open + 1..close])
        .into_iter()
        .map(|text| parse_param(path, &text))
        .collect::<Result<Vec<_>>>()?;

    Ok(SpecMethod {
        return_type,
        name: name.to_string(),
        params,
    })
}

fn parse_param(path: &str, text: &str) -> Result<SpecParam> {
    let text = text.trim();
    let split = text
        .rfind(|c: char| c.is_whitespace() || c == '&' || c == '*')
        .ok_or_else(|| Error::SpecParse {
            path: path.to_string(),
            reason: format!("unnamed parameter `{text}`"),
        })?;
    Ok(SpecParam {
        ty: text[..=split].trim().to_string(),
        name: text[split + 1..].trim().to_string(),
    })
}

fn split_params(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for character in text.chars() {
        match character {
            '<' | '(' | '[' => {
                depth += 1;
                current.push(character);
            }
            '>' | ')' | ']' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_nitrogen_spec_method() {
        let source = "
      virtual std::vector<OutputData> createRandomData(double amount, const std::string& keysetId, const std::optional<std::vector<double>>& customSplit) = 0;
      virtual void reset() = 0;
        ";
        let methods = parse("spec.hpp", source).expect("parse");
        assert_eq!(methods.len(), 2);
        assert_eq!(methods[0].name, "createRandomData");
        assert_eq!(methods[0].return_type, "std::vector<OutputData>");
        assert_eq!(methods[0].params.len(), 3);
        assert_eq!(methods[0].params[2].name, "customSplit");
        assert_eq!(
            methods[0].params[2].ty,
            "const std::optional<std::vector<double>>&"
        );
        assert_eq!(methods[1].name, "reset");
        assert!(methods[1].params.is_empty());
    }

    #[test]
    fn skips_a_virtual_destructor() {
        let source = "
      virtual ~HybridFooSpec() override = default;

    public:
      virtual std::string keysetId() = 0;
        ";
        let methods = parse("spec.hpp", source).expect("parse");
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "keysetId");
        assert_eq!(methods[0].return_type, "std::string");
    }

    #[test]
    fn parses_a_shared_ptr_return() {
        let source = "virtual std::shared_ptr<ArrayBuffer> hash(const std::shared_ptr<ArrayBuffer>& data) = 0;";
        let methods = parse("spec.hpp", source).expect("parse");
        assert_eq!(methods[0].return_type, "std::shared_ptr<ArrayBuffer>");
        assert_eq!(methods[0].name, "hash");
    }
}
