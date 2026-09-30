//! Go naming: `CustomerId` becomes `CustomerID`, the way golint and staticcheck spell initialisms.

const INITIALISMS: &[&str] = &[
    "ACL", "AMQP", "API", "ASCII", "CPU", "CSS", "DB", "DNS", "EOF", "GID", "GUID", "HTML", "HTTP",
    "HTTPS", "ID", "IP", "JSON", "QPS", "RAM", "RPC", "RTP", "SIP", "SLA", "SMTP", "SQL", "SSH",
    "TCP", "TLS", "TS", "TTL", "UDP", "UI", "UID", "URI", "URL", "UTF8", "UUID", "VM", "XML",
    "XMPP", "XSRF", "XSS",
];

/// Rewrites every identifier of a Go name or type expression (`map[string][]UserId`).
pub(crate) fn initialisms(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        identifier(&rest[..end], &mut out);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Words end where a lowercase letter or digit is followed by an uppercase one.
fn identifier(ident: &str, out: &mut String) {
    let bytes = ident.as_bytes();
    let mut start = 0;
    for i in 1..=bytes.len() {
        let boundary = i == bytes.len()
            || bytes[i] == b'_'
            || bytes[i - 1] == b'_'
            || (!bytes[i - 1].is_ascii_uppercase() && bytes[i].is_ascii_uppercase());
        if boundary {
            word(&ident[start..i], start == 0, out);
            start = i;
        }
    }
}

fn word(word: &str, first: bool, out: &mut String) {
    let lower_start = word.starts_with(|c: char| c.is_ascii_lowercase());
    let (stem, plural) = match word.strip_suffix('s') {
        Some(stem) if !is_initialism(word) && stem.len() > 1 && is_initialism(stem) => (stem, "s"),
        _ => (word, ""),
    };
    if !is_initialism(stem) || (first && lower_start && word == word.to_ascii_lowercase()) {
        out.push_str(word);
        return;
    }
    out.push_str(&stem.to_ascii_uppercase());
    out.push_str(plural);
}

fn is_initialism(word: &str) -> bool {
    INITIALISMS.contains(&word.to_ascii_uppercase().as_str())
}

/// Rewrites Markdown for Go doc comments: fenced code becomes an indented block, `[text](url)` a
/// doc link with its definition at the end, and `**bold**` plain text.
pub(crate) fn doc_text(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut links: Vec<(String, String)> = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        if in_code {
            out.push(if line.is_empty() {
                String::new()
            } else {
                format!("\t{line}")
            });
            continue;
        }
        let line = doc_links(line, &mut links).replace("**", "");
        if !line.is_empty() && out.last().is_some_and(|l| l.starts_with('\t')) {
            out.push(String::new());
        }
        out.push(line);
    }
    if !links.is_empty() {
        out.push(String::new());
        out.extend(links.iter().map(|(text, url)| format!("[{text}]: {url}")));
    }
    out.join("\n")
}

fn doc_links(line: &str, links: &mut Vec<(String, String)>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let parsed = rest[open + 1..].split_once("](").and_then(|(text, tail)| {
            let (url, after) = tail.split_once(')')?;
            let plain = !text.contains(['[', ']']) && !url.contains([' ', '(']);
            plain.then_some((text, url, after))
        });
        let Some((text, url, after)) = parsed else {
            out.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
            continue;
        };
        out.push_str(&rest[..open]);
        match links.iter().find(|(t, _)| t == text) {
            _ if text == url => out.push_str(url),
            Some((_, known)) if known != url => out.push_str(&format!("{text} ({url})")),
            known => {
                if known.is_none() {
                    links.push((text.to_owned(), url.to_owned()));
                }
                out.push_str(&format!("[{text}]"));
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{doc_text, initialisms};

    #[test]
    fn spells_initialisms_like_golint() {
        for (name, want) in [
            ("CustomerId", "CustomerID"),
            ("Ids", "IDs"),
            ("PlanIds", "PlanIDs"),
            ("ErrorUri", "ErrorURI"),
            ("NewConfigValueJson", "NewConfigValueJSON"),
            ("HttpsUrl", "HTTPSURL"),
            ("customerId", "customerID"),
            ("id", "id"),
            ("url", "url"),
            ("OAuthErrorResponse", "OAuthErrorResponse"),
            ("CustomerIDRate", "CustomerIDRate"),
            ("Identity", "Identity"),
            ("Status", "Status"),
            ("Utf8Name", "UTF8Name"),
            ("*map[string][]UserId", "*map[string][]UserID"),
            ("RequiredSlice[ApiKey]", "RequiredSlice[APIKey]"),
            ("json.RawMessage", "json.RawMessage"),
        ] {
            assert_eq!(initialisms(name), want, "{name}");
            assert_eq!(initialisms(want), want, "{want} is stable");
        }
    }

    #[test]
    fn markdown_becomes_go_doc_syntax() {
        let text = "See [the docs](https://x.dev/a) or [the docs](https://x.dev/b), **now**.\n\
                    ```go\nclient.Do()\n\nx := 1\n```\nAfter [https://y.dev](https://y.dev) [a]";
        assert_eq!(
            doc_text(text),
            "See [the docs] or the docs (https://x.dev/b), now.\n\n\tclient.Do()\n\n\tx := 1\n\n\
             After https://y.dev [a]\n\n[the docs]: https://x.dev/a"
        );
        assert_eq!(doc_text("plain\n\n- item"), "plain\n\n- item");
    }
}
