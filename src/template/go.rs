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

#[cfg(test)]
mod tests {
    use super::initialisms;

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
}
