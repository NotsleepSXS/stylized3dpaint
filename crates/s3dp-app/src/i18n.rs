//! UI localization: English strings are the keys, `i18n/ru.json` maps them to Russian.
//! Missing translations fall back to English. Russian is the default language (spec §0).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static RUSSIAN: AtomicBool = AtomicBool::new(true);

fn table() -> &'static HashMap<String, String> {
    static T: OnceLock<HashMap<String, String>> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str(include_str!("../i18n/ru.json")).expect("i18n/ru.json"))
}

pub fn is_russian() -> bool {
    RUSSIAN.load(Ordering::Relaxed)
}

pub fn set_russian(on: bool) {
    RUSSIAN.store(on, Ordering::Relaxed);
}

/// Translates an English UI string.
pub fn t(s: &str) -> &str {
    if is_russian() { table().get(s).map(String::as_str).unwrap_or(s) } else { s }
}

/// Picks between an English and an optional Russian variant (effect definitions).
pub fn pick<'a>(en: &'a str, ru: Option<&'a str>) -> &'a str {
    if is_russian() { ru.unwrap_or(en) } else { en }
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_parses_and_translates() {
        super::set_russian(true);
        assert_eq!(super::t("Layers"), "Слои");
        assert_eq!(super::t("no such string"), "no such string");
    }
}
