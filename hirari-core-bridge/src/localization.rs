use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranslationCatalog {
    pub default_locale: String,
    pub strings: HashMap<String, HashMap<String, String>>,
}

impl TranslationCatalog {
    pub fn validate(&self) -> bool {
        !self.default_locale.trim().is_empty()
            && self.default_locale.len() <= 16
            && self
                .strings
                .keys()
                .any(|locale| locale.eq_ignore_ascii_case(&self.default_locale))
            && self.strings.len() <= 65_536
            && self.strings.iter().all(|(locale, values)| {
                !locale.trim().is_empty()
                    && locale.len() <= 16
                    && values.len() <= 65_536
                    && values.iter().all(|(key, value)| {
                        !key.trim().is_empty() && key.len() <= 256 && value.len() <= 16_384
                    })
            })
    }
    pub fn translate<'a>(&'a self, locale: &str, key: &str) -> Option<&'a str> {
        if !self.validate() || key.trim().is_empty() {
            return None;
        }
        self.strings
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(locale))
            .and_then(|(_, m)| lookup_value(m, key))
            .or_else(|| {
                self.strings
                    .iter()
                    .find(|(candidate, _)| candidate.eq_ignore_ascii_case(&self.default_locale))
                    .and_then(|(_, m)| lookup_value(m, key))
            })
            .map(String::as_str)
    }

    pub fn merge_locale(&mut self, locale: &str, values: HashMap<String, String>) -> bool {
        if locale.trim().is_empty()
            || locale.len() > 16
            || locale.contains('\0')
            || values.len() > 65_536
            || values.iter().any(|(k, v)| {
                k.trim().is_empty()
                    || k.len() > 256
                    || v.len() > 16_384
                    || k.contains('\0')
                    || v.contains('\0')
            })
        {
            return false;
        }
        let key = self
            .strings
            .keys()
            .find(|candidate| candidate.eq_ignore_ascii_case(locale))
            .cloned()
            .unwrap_or_else(|| locale.trim().to_owned());
        self.strings.entry(key).or_default().extend(values);
        self.validate()
    }
}

fn lookup_value<'a>(map: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_falls_back_to_default() {
        let mut strings = HashMap::new();
        strings.insert("en".into(), HashMap::from([("play".into(), "Play".into())]));
        let c = TranslationCatalog {
            default_locale: "en".into(),
            strings,
        };
        assert_eq!(c.translate("ja", "play"), Some("Play"));
        assert!(c.translate("en", "missing").is_none());
    }
}
