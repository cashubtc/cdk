//! Rust identifier conventions translated to JavaScript and C++ ones.

/// `snake_case` or `PascalCase` to `lowerCamelCase`.
pub fn lower_camel(name: &str) -> String {
    let pascal = pascal(name);
    let mut chars = pascal.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `snake_case` or `PascalCase` to `PascalCase`.
pub fn pascal(name: &str) -> String {
    if !name.contains('_') && name.chars().next().is_some_and(char::is_uppercase) {
        return name.to_string();
    }
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A C++ identifier that is safe to append to another one.
pub fn cpp_ident(name: &str) -> String {
    pascal(name)
}
