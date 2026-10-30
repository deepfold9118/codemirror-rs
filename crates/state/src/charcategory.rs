//! Character categories. Port of `charcategory.ts`.

/// The categories produced by a [character
/// categorizer](crate::EditorState::char_categorizer). These are used to do
/// things like selecting by word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharCategory {
    /// Word characters.
    Word,
    /// Whitespace.
    Space,
    /// Anything else.
    Other,
}

fn is_space(c: char) -> bool {
    // JavaScript's `\s` also matches the byte order mark.
    c.is_whitespace() || c == '\u{feff}'
}

fn has_word_char(s: &str) -> bool {
    s.chars().any(|c| c.is_alphabetic() || c.is_numeric() || c == '_')
}

/// Create a categorizer that treats the characters in `word_chars` as word
/// characters in addition to letters, numbers and `_`.
pub fn make_categorizer(word_chars: &str) -> impl Fn(&str) -> CharCategory + '_ {
    move |s: &str| {
        if s.chars().all(is_space) {
            CharCategory::Space
        } else if has_word_char(s) || word_chars.chars().any(|w| s.contains(w)) {
            CharCategory::Word
        } else {
            CharCategory::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes() {
        let cat = make_categorizer("$");
        assert_eq!(cat("a"), CharCategory::Word);
        assert_eq!(cat("é"), CharCategory::Word);
        assert_eq!(cat("中"), CharCategory::Word);
        assert_eq!(cat("7"), CharCategory::Word);
        assert_eq!(cat("$"), CharCategory::Word);
        assert_eq!(cat(" "), CharCategory::Space);
        assert_eq!(cat("\t"), CharCategory::Space);
        assert_eq!(cat("."), CharCategory::Other);
    }
}
