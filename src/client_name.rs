//! The client name the SDKs are named after: derived from `info.title`, and kept clear of the
//! names it would break (standard library modules, dependencies, runtime files and types).

use heck::{ToSnakeCase, ToUpperCamelCase};

/// Appended to a name that collides, so `Types` becomes `TypesSdk` (package `types_sdk`).
const SUFFIX: &str = "Sdk";

/// What a title without a usable letter is called.
const FALLBACK: &str = "Sdk";

/// Package, module, crate and file names (snake_case) a client name must not produce: they shadow
/// a standard library module, a dependency, or a file the runtime or generator writes.
const RESERVED_PACKAGES: &[&str] = &[
    // Python standard library, and the dependencies the runtime imports.
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "base64",
    "binascii",
    "builtins",
    "calendar",
    "cgi",
    "cmd",
    "code",
    "codecs",
    "collections",
    "colorsys",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "csv",
    "ctypes",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "dis",
    "email",
    "encodings",
    "enum",
    "errno",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "imaplib",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "math",
    "mimetypes",
    "multiprocessing",
    "numbers",
    "operator",
    "os",
    "pathlib",
    "pickle",
    "platform",
    "pprint",
    "queue",
    "random",
    "re",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtplib",
    "socket",
    "socketserver",
    "sqlite3",
    "ssl",
    "stat",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "sys",
    "sysconfig",
    "tarfile",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "token",
    "tokenize",
    "trace",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uuid",
    "venv",
    "warnings",
    "weakref",
    "webbrowser",
    "xml",
    "zipfile",
    "zlib",
    "zoneinfo",
    "httpx",
    "httpcore",
    "anyio",
    "sniffio",
    "certifi",
    "idna",
    "h11",
    "pydantic",
    "typing_extensions",
    "dateutil",
    "requests",
    "attr",
    "attrs",
    // Go standard library.
    "archive",
    "bufio",
    "bytes",
    "bzip2",
    "compress",
    "container",
    "context",
    "crypto",
    "database",
    "debug",
    "embed",
    "encoding",
    "errors",
    "expvar",
    "flag",
    "fmt",
    "hash",
    "image",
    "index",
    "iter",
    "log",
    "maps",
    "mime",
    "net",
    "os",
    "path",
    "plugin",
    "reflect",
    "regexp",
    "runtime",
    "slices",
    "sort",
    "strconv",
    "strings",
    "structs",
    "sync",
    "syscall",
    "testing",
    "text",
    "unique",
    "unsafe",
    "utf8",
    "utf16",
    // Rust: the standard library, the crates the runtime depends on, and reserved crate names.
    "std",
    "core",
    "alloc",
    "proc_macro",
    "test",
    "serde",
    "serde_json",
    "tokio",
    "hyper",
    "hyper_util",
    "http_body_util",
    "http1",
    "sha2",
    "percent_encoding",
    "reqwest",
    "main",
    "self",
    "super",
    "crate",
    // Files and directories the generator and the runtimes write at the SDK's top level.
    "api",
    "models",
    "types",
    "client",
    "serialization",
    "unions",
    "common",
    "middleware",
    "request",
    "request_auth",
    "request_option",
    "request_pager",
    "request_streaming",
    "nullable",
    "collections",
    "error_status",
    "union_rules",
    "errors",
    "lib",
    "mod",
    "index",
    "init",
    "setup",
    "build",
    "options",
    "pagination",
    "auth",
    "streaming",
    "upload",
    "webhooks",
    "codec",
    "connector",
    "configuration",
    "event_stream",
    "auth_schemes",
    "request_options",
];

/// Client names (UpperCamelCase) that are a builtin or runtime type in some language.
const RESERVED_CLASSES: &[&str] = &[
    "Any",
    "Array",
    "Auth",
    "Boolean",
    "Box",
    "Buffer",
    "Client",
    "Date",
    "Error",
    "Exception",
    "Function",
    "Int",
    "Map",
    "Never",
    "Number",
    "Object",
    "Option",
    "Options",
    "Pager",
    "Promise",
    "Record",
    "Request",
    "RequestOptions",
    "Response",
    "Result",
    "Self",
    "Set",
    "String",
    "Symbol",
    "Task",
    "Thread",
    "Type",
    "Upload",
    "Vec",
    "Void",
];

/// `name` for the title's client: UpperCamelCase ASCII, starting with a letter, never colliding.
pub fn from_title(title: &str) -> String {
    let words: Vec<String> = transliterate(title)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| {
            !w.is_empty()
                && ![
                    "api", "rest", "openapi", "service", "sdk", "sdks", "clients",
                ]
                .contains(&w.to_lowercase().as_str())
        })
        .map(str::to_owned)
        .collect();
    let name = words.join(" ").to_upper_camel_case();
    match name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        true => FALLBACK.into(),
        false => avoid(&name),
    }
}

/// `name`, or `name` with a suffix when it would collide.
pub fn avoid(name: &str) -> String {
    match collision(name) {
        Some(_) => format!("{name}{SUFFIX}"),
        None => name.to_owned(),
    }
}

/// Why `name` can't be a client name, if it can't.
pub fn collision(name: &str) -> Option<String> {
    let snake = name.to_snake_case();
    if RESERVED_PACKAGES.contains(&snake.as_str()) {
        return Some(format!(
            "its package `{snake}` would shadow a standard library module, a dependency or a generated file"
        ));
    }
    // Go packages (and Java's last package segment) join the words without underscores.
    let joined = snake.replace('_', "");
    if RESERVED_PACKAGES.contains(&joined.as_str()) {
        return Some(format!(
            "its Go package `{joined}` would shadow a standard library package or a generated file"
        ));
    }
    // The package segment (`com.public.api`, Go's `package default`) must not be a keyword.
    let keyword = ["rust", "python", "go", "java", "typescript"]
        .into_iter()
        .filter_map(|language| crate::template::ident::keywords(language).ok())
        .flatten()
        .flat_map(|list| list.iter().copied())
        .find(|k| *k == snake || *k == joined);
    if let Some(keyword) = keyword {
        return Some(format!("`{keyword}` is a keyword in some SDK language"));
    }
    if RESERVED_CLASSES.contains(&name) {
        return Some(format!(
            "`{name}` is a builtin or runtime type in some SDK language"
        ));
    }
    None
}

/// `name`, written in any case, as the UpperCamelCase identifier the client is named after, kept
/// as written when it already is one (`AcmeAPI`).
pub fn parse(name: &str) -> Result<String, String> {
    let camel = match name.chars().all(|c| c.is_ascii_alphanumeric()) {
        true if name.starts_with(|c: char| c.is_ascii_uppercase()) => name.to_owned(),
        _ => name.to_upper_camel_case(),
    };
    match camel.starts_with(|c: char| c.is_ascii_alphabetic())
        && camel.chars().all(|c| c.is_ascii_alphanumeric())
    {
        true => check(&camel).map(|()| camel),
        false => Err(format!(
            "`name = {name:?}` must start with a letter and hold ASCII letters, digits, spaces, `-` or `_`"
        )),
    }
}

/// Rejects an explicit `name` that [`collision`] reports, suggesting the suffixed one.
pub fn check(name: &str) -> Result<(), String> {
    match collision(name) {
        Some(why) => Err(format!(
            "`name = {name:?}` can't be used: {why}. Pick another, such as `{name}{SUFFIX}`"
        )),
        None => Ok(()),
    }
}

/// `text` with the Latin letters carrying diacritics as plain ASCII (`ü` as `u`, `ß` as `ss`).
/// What has no ASCII spelling becomes a space, so it separates words.
pub fn transliterate(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        out.push_str(match c {
            'À'..='Å' | 'Ā' | 'Ă' | 'Ą' => "A",
            'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
            'Æ' => "Ae",
            'æ' => "ae",
            'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "C",
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
            'Ď' | 'Đ' | 'Ð' => "D",
            'ď' | 'đ' | 'ð' => "d",
            'È'..='Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "G",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'Ĥ' | 'Ħ' => "H",
            'ĥ' | 'ħ' => "h",
            'Ì'..='Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'Ĵ' => "J",
            'ĵ' => "j",
            'Ķ' => "K",
            'ķ' => "k",
            'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "L",
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
            'Ñ' | 'Ń' | 'Ņ' | 'Ň' => "N",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'Ò'..='Ö' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => "O",
            'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'Œ' => "Oe",
            'œ' => "oe",
            'Ŕ' | 'Ŗ' | 'Ř' => "R",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'Ś' | 'Ŝ' | 'Ş' | 'Š' => "S",
            'ś' | 'ŝ' | 'ş' | 'š' => "s",
            'ß' | 'ẞ' => "ss",
            'Ţ' | 'Ť' | 'Ŧ' => "T",
            'ţ' | 'ť' | 'ŧ' => "t",
            'Þ' => "Th",
            'þ' => "th",
            'Ù'..='Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
            'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'Ŵ' => "W",
            'ŵ' => "w",
            'Ý' | 'Ŷ' | 'Ÿ' => "Y",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'Ź' | 'Ż' | 'Ž' => "Z",
            'ź' | 'ż' | 'ž' => "z",
            _ => " ",
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_camel_case_names() {
        assert_eq!(from_title("Acme Pet Store API"), "AcmePetStore");
        assert_eq!(from_title("acme-billing REST service"), "AcmeBilling");
    }

    #[test]
    fn keywords_get_a_suffix() {
        assert_eq!(from_title("Public API"), "PublicSdk");
        assert_eq!(from_title("Default"), "DefaultSdk");
        assert!(check("Public").is_err());
        assert!(check("Default").is_err());
        assert!(check("Acme").is_ok());
    }

    #[test]
    fn diacritics_are_transliterated() {
        assert_eq!(from_title("Ünïcode Straße"), "UnicodeStrasse");
        assert_eq!(from_title("Café Œuvre"), "CafeOeuvre");
        assert_eq!(from_title("日本 Shop"), "Shop");
    }

    #[test]
    fn titles_without_letters_fall_back() {
        for title in ["!!!", "123 API", "API", "", "日本"] {
            assert_eq!(from_title(title), "Sdk", "{title:?}");
        }
    }

    #[test]
    fn colliding_titles_get_a_suffix() {
        for (title, name) in [
            ("Types", "TypesSdk"),
            ("JSON", "JsonSdk"),
            ("Typing", "TypingSdk"),
            ("Fmt", "FmtSdk"),
            ("Std", "StdSdk"),
            ("Client", "ClientSdk"),
            ("Errors", "ErrorsSdk"),
            ("String", "StringSdk"),
            ("Self", "SelfSdk"),
            ("Serde", "SerdeSdk"),
            ("Httpx", "HttpxSdk"),
            ("Acme", "Acme"),
        ] {
            assert_eq!(from_title(title), name, "{title}");
        }
    }

    #[test]
    fn explicit_names_that_collide_are_rejected() {
        assert!(check("Types").unwrap_err().contains("TypesSdk"));
        assert!(check("Context").is_err());
        assert!(check("Acme").is_ok());
    }
}
