//! Translation shared by the server and the web pages. The server links this
//! crate directly; the pages load it as WebAssembly through `ankiquest-i18n-wasm`,
//! so both sides look up the same catalogs with the same rules.

use std::sync::OnceLock;

mod tables {
    include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
}

/// Languages with a catalog, besides English, which is the source text.
pub const LANGUAGES: [&str; 5] = ["en", "es", "fr", "de", "pt"];

/// The supported language for a tag such as `pt-BR`, or English.
pub fn normalize(language: &str) -> &'static str {
    let code = language.split(['-', '_']).next().unwrap_or("");
    LANGUAGES
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(code))
        .unwrap_or("en")
}

pub struct Catalog {
    values: &'static [Option<&'static str>],
    /// Entries with placeholders, most specific first.
    templates: Vec<(&'static str, &'static str)>,
}

impl Catalog {
    fn new(values: &'static [Option<&'static str>]) -> Self {
        let mut templates: Vec<_> = tables::KEYS
            .iter()
            .zip(values)
            .filter_map(|(key, value)| Some((*key, (*value)?)))
            .filter(|(key, _)| key.contains("{0}"))
            .collect();
        // A broad "Review {0} cards" must not capture "100 mature" as its count.
        templates.sort_by_key(|(source, _)| {
            std::cmp::Reverse(
                source
                    .split('{')
                    .enumerate()
                    .map(|(index, part)| {
                        if index == 0 {
                            part.len()
                        } else {
                            part.split_once('}').map_or(0, |(_, suffix)| suffix.len())
                        }
                    })
                    .sum::<usize>(),
            )
        });
        Self { values, templates }
    }

    /// The translation of exactly this text, if the catalog has one.
    pub fn get(&self, key: &str) -> Option<&'static str> {
        let index = tables::KEYS.binary_search(&key).ok()?;
        self.values[index]
    }
}

pub fn catalog(language: &str) -> Option<&'static Catalog> {
    static CATALOGS: [OnceLock<Catalog>; 4] = [const { OnceLock::new() }; 4];
    let (slot, values) = match normalize(language) {
        "es" => (0, tables::ES),
        "fr" => (1, tables::FR),
        "de" => (2, tables::DE),
        "pt" => (3, tables::PT),
        _ => return None,
    };
    Some(CATALOGS[slot].get_or_init(|| Catalog::new(values)))
}

/// Translates system-authored text on the server, matching templates such as
/// `Review {0} cards` too. Substituted values stay verbatim.
pub fn text(value: &str, language: &str) -> String {
    let Some(catalog) = catalog(language) else {
        return value.into();
    };
    if let Some(exact) = catalog.get(value) {
        return exact.into();
    }
    for (source, target) in &catalog.templates {
        if let Some(values) = capture(source, value) {
            // Replace in one pass, so a name containing a placeholder is never interpreted.
            return substitute(target, &values);
        }
    }
    value.into()
}

fn capture<'a>(pattern: &str, mut value: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = pattern.split('{');
    value = value.strip_prefix(parts.next()?)?;
    let mut values = Vec::new();
    for part in parts {
        let (index, suffix) = part.split_once('}')?;
        if index.parse::<usize>().ok()? != values.len() {
            return None;
        }
        let end = if suffix.is_empty() {
            value.len()
        } else {
            value.find(suffix)?
        };
        values.push(&value[..end]);
        value = &value[end + suffix.len()..];
    }
    value.is_empty().then_some(values)
}

/// Replaces `{n}` with the nth value. Placeholders without a value stay as written.
pub fn substitute(pattern: &str, values: &[&str]) -> String {
    let mut result = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        result.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let value = (digits > 0 && after[digits..].starts_with('}'))
            .then(|| after[..digits].parse::<usize>().ok())
            .flatten()
            .and_then(|index| values.get(index));
        match value {
            Some(value) => {
                result.push_str(value);
                rest = &after[digits + 1..];
            }
            None => {
                result.push('{');
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Translates interface text in a page: surrounding whitespace is kept, and only
/// exact catalog entries apply.
pub fn translate(source: &str, language: &str) -> String {
    let key = source.trim();
    match catalog(language).and_then(|catalog| catalog.get(key)) {
        Some(target) => {
            let leading = &source[..source.len() - source.trim_start().len()];
            let trailing = &source[source.trim_end().len()..];
            format!("{leading}{target}{trailing}")
        }
        None => source.into(),
    }
}

const MARK_START: char = '\u{1}';
const MARK_END: char = '\u{2}';

/// Translates the text and the `aria-label`, `title` and `placeholder` values of
/// an HTML template. The page marks each interpolated value as `\u{1}AQ<n>\u{2}`;
/// within a fragment those become `{0}`, `{1}`, … for the catalog lookup and are
/// put back afterwards, so values are never translated or reordered by accident.
pub fn translate_html(source: &str, language: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while !rest.is_empty() {
        // A tag runs from a `<` to the next `>`; a `<` without one is text.
        let tag = rest
            .find('<')
            .and_then(|open| Some((open, open + rest[open..].find('>')? + 1)));
        let (text, next) = match tag {
            Some((open, end)) => (&rest[..open], Some((&rest[open..end], &rest[end..]))),
            None => (rest, None),
        };
        if text.starts_with('<') {
            out += &translate_attributes(text, language);
        } else if !text.is_empty() {
            out += &fragment(text, language);
        }
        match next {
            Some((tag, after)) => {
                out += &translate_attributes(tag, language);
                rest = after;
            }
            None => break,
        }
    }
    out
}

fn fragment(value: &str, language: &str) -> String {
    let mut slots = Vec::new();
    let mut key = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find(MARK_START) {
        key.push_str(&rest[..start]);
        match marker_at(&rest[start..]) {
            Some(length) => {
                slots.push(&rest[start..start + length]);
                key.push_str(&format!("{{{}}}", slots.len() - 1));
                rest = &rest[start + length..];
            }
            None => {
                key.push(MARK_START);
                rest = &rest[start + MARK_START.len_utf8()..];
            }
        }
    }
    key.push_str(rest);
    substitute(&translate(&key, language), &slots)
}

/// The length of a `\u{1}AQ<digits>\u{2}` marker at the start of `value`.
fn marker_at(value: &str) -> Option<usize> {
    let body = value.strip_prefix(MARK_START)?.strip_prefix("AQ")?;
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0 && body[digits..].starts_with(MARK_END))
        .then_some(MARK_START.len_utf8() + 2 + digits + MARK_END.len_utf8())
}

fn translate_attributes(tag: &str, language: &str) -> String {
    const NAMES: [&str; 3] = ["aria-label", "title", "placeholder"];
    let mut out = String::with_capacity(tag.len());
    let mut index = 0;
    'scan: while index < tag.len() {
        let boundary = tag[..index]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if boundary {
            for name in NAMES {
                let Some(after) = tag[index..]
                    .strip_prefix(name)
                    .and_then(|rest| rest.strip_prefix("=\""))
                else {
                    continue;
                };
                let Some(close) = after.find('"') else {
                    continue;
                };
                out += name;
                out += "=\"";
                out += &fragment(&after[..close], language);
                out.push('"');
                index += name.len() + 2 + close + 1;
                continue 'scan;
            }
        }
        let c = tag[index..].chars().next().unwrap();
        out.push(c);
        index += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(index: usize) -> String {
        format!("\u{1}AQ{index}\u{2}")
    }

    #[test]
    fn languages_fall_back_to_english() {
        assert_eq!(normalize("pt-BR"), "pt");
        assert_eq!(normalize("DE_at"), "de");
        assert_eq!(normalize("ja"), "en");
        assert_eq!(normalize(""), "en");
        assert!(catalog("en").is_none());
    }

    #[test]
    fn every_catalog_has_the_same_templates_filtered_from_its_own_entries() {
        for language in &LANGUAGES[1..] {
            let catalog = catalog(language).unwrap();
            assert!(catalog.templates.iter().all(|(key, _)| key.contains("{0}")));
            assert!(!catalog.templates.is_empty());
        }
    }

    #[test]
    fn substitute_leaves_unknown_and_malformed_placeholders() {
        assert_eq!(substitute("{0} and {1}", &["a", "b"]), "a and b");
        assert_eq!(substitute("{1} before {0}", &["a", "b"]), "b before a");
        assert_eq!(
            substitute("{2} {x} {} {+0} {{0}", &["a"]),
            "{2} {x} {} {+0} {a"
        );
        assert_eq!(
            substitute("{0}", &["{1}"]),
            "{1}",
            "values are never rescanned"
        );
    }

    #[test]
    fn translate_keeps_surrounding_whitespace_and_needs_an_exact_entry() {
        let (key, target) = tables::KEYS
            .iter()
            .zip(tables::ES)
            .find_map(|(key, value)| Some((*key, (*value)?)))
            .unwrap();
        assert_eq!(
            translate(&format!("  {key}\n"), "es"),
            format!("  {target}\n")
        );
        assert_eq!(translate(key, "en"), key);
        assert_eq!(translate("not in any catalog", "es"), "not in any catalog");
    }

    #[test]
    fn html_translates_text_and_labelling_attributes_but_not_values() {
        let key = tables::KEYS
            .iter()
            .zip(tables::ES)
            .find(|(key, value)| key.contains("{0}") && !key.contains("{1}") && value.is_some())
            .map(|(key, _)| *key)
            .unwrap();
        let target = catalog("es").unwrap().get(key).unwrap();
        let source = format!(
            "<p class=x title=\"{}\" data-id=\"{}\">{}</p>",
            substitute(key, &[&marker(0)]),
            marker(1),
            substitute(key, &[&marker(2)])
        );
        let expected = format!(
            "<p class=x title=\"{}\" data-id=\"{}\">{}</p>",
            substitute(target, &[&marker(0)]),
            marker(1),
            substitute(target, &[&marker(2)])
        );
        assert_eq!(translate_html(&source, "es"), expected);
        assert_eq!(translate_html(&source, "en"), source);
        assert_eq!(translate_html("a < b", "es"), "a < b");
        assert_eq!(translate_html("<unclosed", "es"), "<unclosed");
        let unclosed = format!("{} <", substitute(key, &[&marker(0)]));
        assert_eq!(
            translate_html(&unclosed, "es"),
            unclosed,
            "a stray < stays part of the text, which then has no entry"
        );
    }
}
