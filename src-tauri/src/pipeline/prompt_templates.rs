//! Presentation prose follows the interface language. Protocol identifiers stay stable.
use serde_json::Value;
use std::sync::LazyLock;

static ZH: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("../../prompts/presentation/zh.json")).expect("valid zh prompt templates"));
static EN: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("../../prompts/presentation/en.json")).expect("valid en prompt templates"));
static JA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("../../prompts/presentation/ja.json")).expect("valid ja prompt templates"));

pub(crate) fn language(lang: &str) -> &'static str {
    match lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase().as_str() {
        "en" => "en", "ja" => "ja", _ => "zh",
    }
}

pub(crate) fn text(lang: &str, key: &str) -> &'static str {
    let catalog = match language(lang) { "en" => &*EN, "ja" => &*JA, _ => &*ZH };
    catalog[key].as_str().unwrap_or_else(|| panic!("missing prompt template: {key}"))
}

/// Read the explicit system-owned locale marker, never infer it from user text.
pub(crate) fn dialogue_language<'a>(system_contents: impl Iterator<Item=&'a str>) -> &'static str {
    let mut selected = "en";
    for content in system_contents {
        if content.trim_end().ends_with("[/REPLY LANGUAGE]") {
            for lang in ["zh","en","ja"] {
                if content.contains(&format!("[REPLY LANGUAGE]\n[LANGUAGE: {lang}]")) { selected = lang; }
            }
        }
    }
    selected
}

/// Substitute only the original template, never reinterpret placeholders in user-authored text.
pub(crate) fn render(lang: &str, key: &str, values: &[(&str, &str)]) -> String {
    let mut remaining = text(lang, key);
    let mut result = String::new();
    while let Some(start) = remaining.find('{') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find('}') else { break; };
        let name = &remaining[1..end];
        if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) {
            result.push_str(value);
        } else { result.push_str(&remaining[..=end]); }
        remaining = &remaining[end + 1..];
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_locales_have_identical_keys_and_placeholders() {
        fn fields(text: &str) -> Vec<&str> {
            let mut result = text.split('{').skip(1).filter_map(|part|part.split_once('}').map(|(key,_)|key)).collect::<Vec<_>>();
            result.sort(); result
        }
        for catalog in [&*ZH, &*JA] {
            assert_eq!(catalog.as_object().unwrap().keys().collect::<Vec<_>>(), EN.as_object().unwrap().keys().collect::<Vec<_>>());
            for (key, english) in EN.as_object().unwrap() {
                assert!(!catalog[key].as_str().unwrap().trim().is_empty());
                assert_eq!(fields(catalog[key].as_str().unwrap()), fields(english.as_str().unwrap()), "{key}");
            }
        }
        assert_eq!(language("EN_us"),"en"); assert_eq!(language("ja-JP"),"ja"); assert_eq!(language("zh-CN"),"zh");
    }
    #[test]
    fn substitution_does_not_execute_placeholders_in_supplied_content() {
        let rendered = render("en", "character_note", &[("name","{speech}"),("personality","{name}"),("speech","actual speech")]);
        assert!(rendered.contains("{speech}")); assert!(rendered.contains("{name}"));
    }
    #[test]
    fn joined_post_history_selects_the_last_explicit_locale() {
        let joined = format!("Character note\n{}", text("ja", "reply_language"));
        assert_eq!(dialogue_language([text("zh", "reply_language"), joined.as_str()].into_iter()), "ja");
        assert_eq!(dialogue_language(["Unmarked legacy fixture"].into_iter()), "en");
    }
}
