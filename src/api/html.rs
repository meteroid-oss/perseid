//! HTML in spec descriptions (Stripe's `<p>`, `<code>`, `<a href>`), as the Markdown doc comments
//! of every language read well.

use std::borrow::Cow;

const TAGS: [&str; 36] = [
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "li",
    "ol",
    "p",
    "pre",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

/// The Markdown of a description that may hold HTML.
pub(crate) fn doc(text: Option<String>) -> Option<String> {
    text.map(|t| match to_markdown(&t) {
        Cow::Borrowed(_) => t,
        Cow::Owned(markdown) => markdown,
    })
}

/// `text` with the HTML tags it uses converted to Markdown, entities decoded and blank lines
/// collapsed. Unknown tags such as `List<T>` are kept.
pub(crate) fn to_markdown(text: &str) -> Cow<'_, str> {
    if !has_html(text) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut links: Vec<Option<String>> = Vec::new();
    let mut lists: Vec<Option<usize>> = Vec::new();
    let mut pre = false;
    let mut rest = text;
    while let Some(start) = rest.find(['<', '&']) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with('&') {
            match entity(rest) {
                Some((decoded, len)) => {
                    out.push_str(&decoded);
                    rest = &rest[len..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
            continue;
        }
        let Some(tag) = Tag::parse(rest) else {
            out.push('<');
            rest = &rest[1..];
            continue;
        };
        rest = &rest[tag.len..];
        match (tag.name.as_str(), tag.closing) {
            ("p" | "div" | "blockquote" | "table" | "dl", _) => out.push_str("\n\n"),
            ("tr" | "dt" | "dd", _) => out.push('\n'),
            ("br", _) => out.push('\n'),
            ("hr", _) => out.push_str("\n\n---\n\n"),
            ("pre", false) => {
                pre = true;
                out.push_str("\n\n```\n");
            }
            ("pre", true) => {
                pre = false;
                out.push_str("\n```\n\n");
            }
            ("code", _) if !pre => out.push('`'),
            ("strong" | "b", _) => out.push_str("**"),
            ("em" | "i", _) => out.push('*'),
            ("a", false) => {
                let href = tag.href.filter(|h| !h.is_empty());
                if href.is_some() {
                    out.push('[');
                }
                links.push(href);
            }
            ("a", true) => {
                if let Some(href) = links.pop().flatten() {
                    out.push_str(&format!("]({href})"));
                }
            }
            ("ul", false) => {
                lists.push(None);
                out.push('\n');
            }
            ("ol", false) => {
                lists.push(Some(0));
                out.push('\n');
            }
            ("ul" | "ol", true) => {
                lists.pop();
                out.push_str("\n\n");
            }
            ("li", false) => {
                let indent = "  ".repeat(lists.len().saturating_sub(1));
                match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        out.push_str(&format!("\n{indent}{n}. "));
                    }
                    _ => out.push_str(&format!("\n{indent}- ")),
                }
            }
            (h, false) if h.len() == 2 && h.starts_with('h') => {
                let level = h[1..].parse().unwrap_or(1);
                out.push_str(&format!("\n\n{} ", "#".repeat(level)));
            }
            (h, true) if h.len() == 2 && h.starts_with('h') => out.push_str("\n\n"),
            _ => {}
        }
    }
    out.push_str(rest);
    Cow::Owned(tidy(&out))
}

fn has_html(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find(['<', '&']) {
        rest = &rest[start..];
        if rest.starts_with('&') && entity(rest).is_some() || Tag::parse(rest).is_some() {
            return true;
        }
        rest = &rest[1..];
    }
    false
}

/// Trims trailing spaces and collapses blank lines, keeping those of code blocks.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.trim().lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

struct Tag {
    name: String,
    closing: bool,
    href: Option<String>,
    len: usize,
}

impl Tag {
    /// The known HTML tag `text` starts with.
    fn parse(text: &str) -> Option<Self> {
        let inner = text.strip_prefix('<')?;
        let (closing, inner) = match inner.strip_prefix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let name_len = inner
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(inner.len());
        let name = inner[..name_len].to_ascii_lowercase();
        if !TAGS.contains(&name.as_str()) {
            return None;
        }
        let after = &inner[name_len..];
        if !after.starts_with(['>', ' ', '/', '\n', '\t']) {
            return None;
        }
        let end = after.find('>')?;
        let attributes = &after[..end];
        if attributes.contains('<') {
            return None;
        }
        let href = attributes.find("href=").and_then(|i| {
            let value = &attributes[i + 5..];
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = &value[1..];
            Some(value[..value.find(quote)?].to_owned())
        });
        Some(Self {
            name,
            closing,
            href,
            len: text.len() - after.len() + end + 1,
        })
    }
}

/// The character an entity `text` starts with stands for, and the entity's length.
fn entity(text: &str) -> Option<(String, usize)> {
    let end = text[..text.len().min(12)].find(';')?;
    let name = &text[1..end];
    let decoded = match name {
        "amp" => "&".to_owned(),
        "lt" => "<".to_owned(),
        "gt" => ">".to_owned(),
        "quot" => "\"".to_owned(),
        "apos" => "'".to_owned(),
        "nbsp" => " ".to_owned(),
        "mdash" => "—".to_owned(),
        "ndash" => "–".to_owned(),
        "hellip" => "…".to_owned(),
        "lsquo" => "‘".to_owned(),
        "rsquo" => "’".to_owned(),
        "ldquo" => "“".to_owned(),
        "rdquo" => "”".to_owned(),
        "copy" => "©".to_owned(),
        "reg" => "®".to_owned(),
        "trade" => "™".to_owned(),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)?.to_string()
        }
    };
    Some((decoded, end + 1))
}

#[cfg(test)]
mod tests {
    use super::to_markdown;

    #[test]
    fn stripe_html_becomes_markdown() {
        assert_eq!(
            to_markdown(
                "<p>Creates a <code>Customer</code>. See <a href=\"https://stripe.com/docs\">the docs</a>.</p>\n<p>Tom &amp; Jerry&#39;s</p>"
            ),
            "Creates a `Customer`. See [the docs](https://stripe.com/docs).\n\nTom & Jerry's"
        );
        assert_eq!(
            to_markdown("<p>One of:</p><ul><li><strong>a</strong></li><li>b</li></ul><p>End</p>"),
            "One of:\n\n- **a**\n- b\n\nEnd"
        );
        assert_eq!(
            to_markdown("Steps<ol><li>one</li><li>two</li></ol>"),
            "Steps\n\n1. one\n2. two"
        );
    }

    #[test]
    fn text_without_html_is_unchanged() {
        for text in [
            "Returns a List<Customer> when a < b",
            "R&D & co",
            "x\n\n\n\ny",
        ] {
            assert_eq!(to_markdown(text), text);
        }
        assert_eq!(to_markdown("<T> &lt;T&gt;"), "<T> <T>");
    }
}
