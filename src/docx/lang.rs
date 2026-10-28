//! Document language for the DOCX renderer.
//!
//! The front matter key `lang` sets the language tag of the document. The
//! tag must match the grammar of RFC 5646 section 2.1. The check is ASCII and
//! ignores case. It does not use the subtag registry, and it rejects the
//! irregular grandfathered tags (for example `i-klingon`).

use anyhow::{bail, Result};
use serde_json::{Map as JsonMap, Value as JsonValue};

/// The language tag when the front matter has no `lang`.
const DEFAULT_LANG: &str = "en";

/// Gives the document language tag from the metadata. An absent or null
/// `lang` gives `en`. A valid tag is given as written.
pub(super) fn document_lang(metadata: &JsonMap<String, JsonValue>) -> Result<&str> {
    match metadata.get("lang") {
        None | Some(JsonValue::Null) => Ok(DEFAULT_LANG),
        Some(JsonValue::String(tag)) if is_language_tag(tag) => Ok(tag),
        Some(JsonValue::String(tag)) => bail!("lang: invalid language tag '{tag}'"),
        Some(_) => bail!("lang: the language tag must be a string"),
    }
}

/// Checks a tag against the `langtag` and `privateuse` rules of RFC 5646.
fn is_language_tag(tag: &str) -> bool {
    let mut subtags = tag.split('-').peekable();
    let Some(first) = subtags.next() else {
        return false;
    };
    if first.eq_ignore_ascii_case("x") {
        return is_private_use_rest(subtags);
    }

    // language = 2*3ALPHA ["-" extlang] / 4ALPHA / 5*8ALPHA
    if !(2..=8).contains(&first.len()) || !is_alpha(first) {
        return false;
    }
    if first.len() <= 3 {
        // extlang = 3ALPHA *2("-" 3ALPHA)
        for _ in 0..3 {
            if subtags.next_if(|s| s.len() == 3 && is_alpha(s)).is_none() {
                break;
            }
        }
    }
    // script = 4ALPHA
    subtags.next_if(|s| s.len() == 4 && is_alpha(s));
    // region = 2ALPHA / 3DIGIT
    subtags.next_if(|s| (s.len() == 2 && is_alpha(s)) || (s.len() == 3 && is_digit(s)));
    // variant = 5*8alphanum / (DIGIT 3alphanum)
    while subtags
        .next_if(|s| {
            is_alphanum(s)
                && ((5..=8).contains(&s.len())
                    || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit()))
        })
        .is_some()
    {}
    // extension = singleton 1*("-" (2*8alphanum))
    while subtags
        .next_if(|s| s.len() == 1 && is_alphanum(s) && !s.eq_ignore_ascii_case("x"))
        .is_some()
    {
        let mut count = 0;
        while subtags
            .next_if(|s| (2..=8).contains(&s.len()) && is_alphanum(s))
            .is_some()
        {
            count += 1;
        }
        if count == 0 {
            return false;
        }
    }
    // privateuse = "x" 1*("-" (1*8alphanum))
    match subtags.next() {
        None => true,
        Some(s) if s.eq_ignore_ascii_case("x") => is_private_use_rest(subtags),
        Some(_) => false,
    }
}

/// Checks the subtags after `x`: one or more subtags of 1 to 8 letters or
/// digits.
fn is_private_use_rest<'a>(subtags: impl Iterator<Item = &'a str>) -> bool {
    let mut count = 0;
    for s in subtags {
        if !(1..=8).contains(&s.len()) || !is_alphanum(s) {
            return false;
        }
        count += 1;
    }
    count > 0
}

fn is_alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

fn is_digit(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

fn is_alphanum(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lang_of(value: Option<JsonValue>) -> Result<String> {
        let mut metadata = JsonMap::new();
        if let Some(value) = value {
            metadata.insert("lang".to_string(), value);
        }
        document_lang(&metadata).map(str::to_string)
    }

    #[test]
    fn valid_tags() {
        for tag in [
            "en",
            "ru-RU",
            "zh-Hant-TW",
            "sr-Latn-RS",
            "de-CH-1996",
            "es-419",
            "en-US-x-twain",
            "x-whatever",
            "EN-us",
            "zh-yue-HK",
            "art-lojban",
            "en-a-bbb-x-a-ccc",
            "hy-Latn-IT-arevela",
            "abcde",
        ] {
            assert!(is_language_tag(tag), "{tag}");
            assert_eq!(lang_of(Some(json!(tag))).unwrap(), tag);
        }
    }

    #[test]
    fn invalid_tags() {
        for tag in [
            "ru_RU",
            "",
            "e",
            "en-",
            "-en",
            "en--US",
            "123",
            "abcdefghi",
            "ру",
            "ru-РУ",
            "x",
            "en-x",
            "en-a",
            "en-a-x-y",
            "i-klingon",
            "en-GB-oed",
            "en-US-US",
            "x-abcdefghi",
        ] {
            assert!(!is_language_tag(tag), "{tag}");
            let err = lang_of(Some(json!(tag))).unwrap_err().to_string();
            assert!(err.starts_with("lang: invalid language tag"), "{err}");
        }
    }

    #[test]
    fn absent_or_null_gives_default() {
        assert_eq!(lang_of(None).unwrap(), "en");
        assert_eq!(lang_of(Some(JsonValue::Null)).unwrap(), "en");
    }

    #[test]
    fn non_string_is_an_error() {
        for value in [json!(1), json!(true), json!(["en"])] {
            let err = lang_of(Some(value)).unwrap_err().to_string();
            assert_eq!(err, "lang: the language tag must be a string");
        }
    }
}
