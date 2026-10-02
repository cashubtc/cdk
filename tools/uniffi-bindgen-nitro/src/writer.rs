//! A tiny indent-aware string builder shared by the emitters.

/// Accumulates generated source with block indentation.
#[derive(Debug, Default)]
pub struct Source {
    text: String,
    indent: usize,
}

impl Source {
    /// A new empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one line at the current indentation.
    pub fn line(&mut self, text: impl AsRef<str>) -> &mut Self {
        let text = text.as_ref();
        if !text.is_empty() {
            for _ in 0..self.indent {
                self.text.push_str("  ");
            }
            self.text.push_str(text);
        }
        self.text.push('\n');
        self
    }

    /// Append a blank line.
    pub fn blank(&mut self) -> &mut Self {
        self.text.push('\n');
        self
    }

    /// Append each line of a block, keeping the current indentation.
    pub fn lines(&mut self, block: &str) -> &mut Self {
        for line in block.lines() {
            self.line(line);
        }
        self
    }

    /// Increase indentation for the lines written by `body`.
    pub fn indented<R>(&mut self, body: impl FnOnce(&mut Self) -> R) -> &mut Self {
        self.indent += 1;
        body(self);
        self.indent -= 1;
        self
    }

    /// The accumulated text.
    pub fn finish(self) -> String {
        self.text
    }

    /// The accumulated text, leaving the buffer usable.
    pub fn clone_text(&self) -> String {
        self.text.clone()
    }
}

/// The banner every generated file starts with.
pub fn banner(comment: &str, crate_name: &str) -> String {
    format!(
        "{comment} GENERATED FILE.\n{comment} DO NOT EDIT.\n{comment}\n\
         {comment} Produced by uniffi-bindgen-nitro from the UniFFI metadata of `{crate_name}`.\n\
         {comment} Change the Rust `#[uniffi::export]` surface and regenerate instead.\n"
    )
}
