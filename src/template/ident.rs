//! Identifiers from spec names, shared by every template: `ident` converts one name, `idents`
//! converts a list and fails when two names become the same identifier.

use heck::{
    ToLowerCamelCase as _, ToShoutySnakeCase as _, ToSnakeCase as _, ToUpperCamelCase as _,
};
use minijinja::{Error, ErrorKind};

const RUST: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "gen", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];
/// Rust keywords that cannot be raw identifiers.
const RUST_NOT_RAW: &[&str] = &["self", "Self", "super", "crate"];
const PYTHON: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "match", "case",
];
const GO: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];
const JAVA: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "var",
    "void",
    "volatile",
    "while",
    "yield",
    "record",
];
/// Reserved words, which cannot name variables or parameters (they can name properties).
const TYPESCRIPT: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
    "await",
];

fn keywords(language: &str) -> Result<&'static [&'static str], Error> {
    Ok(match language {
        "rust" => RUST,
        "python" => PYTHON,
        "go" => GO,
        "java" => JAVA,
        "typescript" => TYPESCRIPT,
        other => {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("unknown identifier language `{other}`"),
            ));
        }
    })
}

/// `name` as a `case` (`snake`, `camel`, `pascal` or `shouty`) identifier. Comparison operators
/// become words (`created<` is `created_lt`), other punctuation separates words, a leading digit
/// gets a `value` prefix and, given a `language`, its keywords get escaped.
pub(crate) fn ident(name: &str, case: &str, language: Option<&str>) -> Result<String, Error> {
    let mut words = String::with_capacity(name.len());
    let mut chars = name.chars().peekable();
    let mut previous = None;
    while let Some(c) = chars.next() {
        let negative = c == '-'
            && chars.peek().is_some_and(char::is_ascii_digit)
            && !previous.is_some_and(char::is_alphanumeric);
        previous = Some(c);
        match c {
            _ if negative => words.push_str(" minus "),
            '<' => words.push_str(" lt "),
            '>' => words.push_str(" gt "),
            '=' => words.push_str(" eq "),
            '+' => words.push_str(" plus "),
            c if c.is_alphanumeric() => words.push(c),
            _ => words.push(' '),
        }
    }
    let convert = |s: &str| -> Result<String, Error> {
        Ok(match case {
            "snake" => s.to_snake_case(),
            "camel" => s.to_lower_camel_case(),
            "pascal" => s.to_upper_camel_case(),
            "shouty" => s.to_shouty_snake_case(),
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("unknown identifier case `{other}`"),
                ));
            }
        })
    };
    let mut out = convert(&words)?;
    if out.is_empty() {
        out = convert("empty")?;
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        out = convert(&format!("value {words}"))?;
    }
    if let Some(language) = language
        && keywords(language)?.contains(&out.as_str())
    {
        out = match language == "rust" && !RUST_NOT_RAW.contains(&out.as_str()) {
            true => format!("r#{out}"),
            false => format!("{out}_"),
        };
    }
    Ok(out)
}

/// [`ident`] of every name, failing when two names of `owner` become the same identifier.
pub(crate) fn idents(
    names: &[String],
    case: &str,
    language: Option<&str>,
    owner: &str,
) -> Result<Vec<String>, Error> {
    let mut seen: Vec<(String, &str)> = Vec::with_capacity(names.len());
    for name in names {
        let ident = ident(name, case, language)?;
        if let Some((_, other)) = seen.iter().find(|(i, _)| *i == ident) {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!(
                    "{owner}: `{other}` and `{name}` both become the identifier `{ident}`{}, \
                     rename one of them in the spec",
                    language.map(|l| format!(" in {l}")).unwrap_or_default()
                ),
            ));
        }
        seen.push((ident, name));
    }
    Ok(seen.into_iter().map(|(ident, _)| ident).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust(name: &str) -> String {
        ident(name, "snake", Some("rust")).unwrap()
    }

    #[test]
    fn keywords_are_escaped_per_language() {
        assert_eq!(rust("type"), "r#type");
        assert_eq!(rust("self"), "self_");
        assert_eq!(ident("class", "snake", Some("python")).unwrap(), "class_");
        assert_eq!(ident("default", "camel", Some("java")).unwrap(), "default_");
        assert_eq!(ident("type", "camel", Some("go")).unwrap(), "type_");
        assert_eq!(ident("Type", "pascal", Some("go")).unwrap(), "Type");
        assert_eq!(ident("default", "camel", None).unwrap(), "default");
    }

    #[test]
    fn punctuation_digits_and_empty_names_become_identifiers() {
        assert_eq!(rust("DateCreated<"), "date_created_lt");
        assert_eq!(rust("DateCreated>="), "date_created_gt_eq");
        assert_eq!(rust("$dollar"), "dollar");
        assert_eq!(ident("+1", "pascal", None).unwrap(), "Plus1");
        assert_eq!(ident("-1", "pascal", None).unwrap(), "Minus1");
        assert_eq!(
            ident("reactions--1", "snake", None).unwrap(),
            "reactions_minus_1"
        );
        assert_eq!(ident("a-1", "snake", None).unwrap(), "a_1");
        assert_eq!(rust("kebab-case"), "kebab_case");
        assert_eq!(rust("1leading"), "value_1leading");
        assert_eq!(ident("3d", "pascal", None).unwrap(), "Value3d");
        assert_eq!(ident("", "pascal", None).unwrap(), "Empty");
        assert_eq!(ident("a.b", "shouty", None).unwrap(), "A_B");
    }

    #[test]
    fn colliding_names_are_reported_with_their_owner() {
        let names = ["type".to_owned(), "@type".to_owned()];
        let error = idents(&names, "snake", Some("rust"), "schema `Reserved`").unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(
                "schema `Reserved`: `type` and `@type` both become the identifier `r#type` in rust"
            ),
            "{message}"
        );
        let names = ["a".to_owned(), "b".to_owned()];
        assert_eq!(idents(&names, "pascal", None, "x").unwrap(), ["A", "B"]);
    }
}
