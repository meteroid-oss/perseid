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
    // Not keywords, but names that linters reject as ambiguous (E741).
    "I", "O", "l",
];
/// Python names that, as a class member or parameter, shadow a name the generated annotations use
/// (`str`, `datetime`, `t`), or are `self` and `cls`. Builtins the generated class bodies never
/// reference (`type`, `object`, `set`) stay usable: they are common JSON member names.
const PYTHON_SHADOWING: &[&str] = &[
    "self", "cls", "str", "bool", "int", "float", "bytes", "datetime", "t",
];
/// Python names only model fields must avoid: the builtin generics a model body uses (`list`
/// operations are annotated `t.List`), and the members `BaseModel` defines.
const PYTHON_FIELD: &[&str] = &[
    "list",
    "dict",
    "tuple",
    "to_dict",
    "to_json",
    "from_dict",
    "from_json",
    "extra_fields",
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

pub(crate) fn keywords(language: &str) -> Result<Vec<&'static [&'static str]>, Error> {
    Ok(match language {
        "rust" => vec![RUST],
        "python" => vec![PYTHON, PYTHON_SHADOWING],
        "python_field" => vec![PYTHON, PYTHON_SHADOWING, PYTHON_FIELD],
        "go" => vec![GO],
        "java" => vec![JAVA],
        "typescript" => vec![TYPESCRIPT],
        other => {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("unknown identifier language `{other}`"),
            ));
        }
    })
}

/// `name` as the value of a Go `json:"..."` struct tag. `encoding/json` silently ignores tags whose
/// name has a quote, backslash, comma or other punctuation outside its allowed set, so such a name
/// is rejected instead of generating a struct that serializes under the wrong key. Plain structs
/// check `go_taggable` first and encode such fields by hand.
pub(crate) fn go_tag(name: &str) -> Result<String, Error> {
    const ALLOWED: &str = "!#$%&()*+-./:;<=>?@[]^_{|}~ ";
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || ALLOWED.contains(c))
    {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            format!(
                "go codegen: the property name {name:?} cannot be a `json` struct tag \
                 (quotes, backslashes, commas and control characters are not allowed), \
                 rename it in the spec"
            ),
        ));
    }
    Ok(name.to_owned())
}

/// `name` as a `case` (`snake`, `camel`, `pascal` or `shouty`) identifier. Comparison operators
/// become words (`created<` is `created_lt`), other punctuation separates words, a leading digit
/// gets a `value` prefix and, given a `language`, its keywords get escaped.
pub(crate) fn ident(name: &str, case: &str, language: Option<&str>) -> Result<String, Error> {
    if let Some(words) = symbol_words(name) {
        return ident(&words, case, language);
    }
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
    if language == Some("go") && case == "pascal" && !out.starts_with(|c: char| c.is_uppercase()) {
        // A first rune that is not an uppercase letter would leave the field unexported.
        out.insert(0, 'X');
    }
    if let Some(language) = language
        && keywords(language)?
            .iter()
            .any(|set| set.contains(&out.as_str()))
    {
        out = match language == "rust" && !RUST_NOT_RAW.contains(&out.as_str()) {
            true => format!("r#{out}"),
            false => format!("{out}_"),
        };
    }
    Ok(out)
}

/// The words naming a value made only of punctuation (`-` is `minus`, `*` is `star`), so that such
/// values stay distinct instead of all becoming `empty`. Operators `ident` already spells (`+`,
/// `<`, `>`, `=`) and values with any letter or digit are left to it.
fn symbol_words(name: &str) -> Option<String> {
    if name.is_empty() || name.chars().any(char::is_alphanumeric) {
        return None;
    }
    let words: Option<Vec<&str>> = name
        .chars()
        .map(|c| {
            Some(match c {
                '-' => "minus",
                '*' => "star",
                '/' => "slash",
                '\\' => "backslash",
                '.' => "dot",
                ',' => "comma",
                ':' => "colon",
                ';' => "semicolon",
                '_' => "underscore",
                '#' => "hash",
                '@' => "at",
                '&' => "and",
                '%' => "percent",
                '!' => "bang",
                '?' => "question",
                '~' => "tilde",
                '^' => "caret",
                '|' => "pipe",
                '$' => "dollar",
                '+' => "plus",
                '<' => "lt",
                '>' => "gt",
                '=' => "eq",
                '(' => "lparen",
                ')' => "rparen",
                '[' => "lbracket",
                ']' => "rbracket",
                '{' => "lbrace",
                '}' => "rbrace",
                '\'' => "apostrophe",
                '"' => "dquote",
                '`' => "backtick",
                _ => return None,
            })
        })
        .collect();
    words.map(|w| w.join(" "))
}

/// The names to derive the identifiers of enum `values` from: the values themselves, except that
/// one whose identifier another earlier value already has (`a-b` and `a_b`, `Active` and `active`)
/// gets a number suffix, so every enum member is named while the wire values stay as they are.
pub(crate) fn enum_names(values: &[String]) -> Vec<String> {
    const CASES: [&str; 3] = ["pascal", "shouty", "snake"];
    let key = |name: &str| -> Vec<String> {
        CASES
            .iter()
            .map(|case| ident(name, case, None).unwrap_or_default())
            .collect()
    };
    let mut used: Vec<Vec<String>> = Vec::with_capacity(values.len());
    let mut names = Vec::with_capacity(values.len());
    for value in values {
        let mut name = value.clone();
        let mut n = 1;
        while used
            .iter()
            .any(|u| u.iter().zip(key(&name)).any(|(a, b)| *a == b))
        {
            n += 1;
            name = format!("{value} {n}");
        }
        used.push(key(&name));
        names.push(name);
    }
    names
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
    fn python_members_avoid_shadowing() {
        let field = |n: &str| ident(n, "snake", Some("python_field")).unwrap();
        let method = |n: &str| ident(n, "snake", Some("python")).unwrap();
        assert_eq!(method("str"), "str_");
        assert_eq!(method("self"), "self_");
        assert_eq!(method("list"), "list");
        assert_eq!(field("list"), "list_");
        assert_eq!(field("str"), "str_");
        assert_eq!(field("to_dict"), "to_dict_");
        assert_eq!(field("toJson"), "to_json_");
        assert_eq!(field("extra_fields"), "extra_fields_");
        assert_eq!(field("name"), "name");
        assert_eq!(field("type"), "type");
        assert_eq!(field("object"), "object");
        assert_eq!(field("tuple"), "tuple_");
    }

    #[test]
    fn go_fields_are_exported_and_tags_checked() {
        assert_eq!(ident("名前", "pascal", Some("go")).unwrap(), "X名前");
        assert_eq!(ident("name", "pascal", Some("go")).unwrap(), "Name");
        assert_eq!(go_tag("user-id").unwrap(), "user-id");
        assert_eq!(go_tag("名前").unwrap(), "名前");
        assert!(go_tag("say\"hi").is_err());
        assert!(go_tag("back\\slash").is_err());
        assert!(go_tag("a,b").is_err());
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
    fn symbols_are_named_and_enum_collisions_get_numbers() {
        assert_eq!(ident("-", "pascal", None).unwrap(), "Minus");
        assert_eq!(ident("*", "pascal", None).unwrap(), "Star");
        assert_eq!(ident("/", "shouty", None).unwrap(), "SLASH");
        let names = |v: &[&str]| enum_names(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(names(&["a-b", "a_b"]), ["a-b", "a_b 2"]);
        assert_eq!(names(&["Active", "active"]), ["Active", "active 2"]);
        assert_eq!(names(&["x", "y"]), ["x", "y"]);
        let all = names(&["a-b", "a_b", "A B"]);
        assert!(idents(&all, "pascal", None, "x").is_ok());
        assert!(idents(&all, "shouty", None, "x").is_ok());
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
