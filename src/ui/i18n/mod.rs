mod en_us;
mod zh_cn;

use std::env;

pub struct Messages {
    #[cfg_attr(not(test), allow(dead_code))]
    pub locale: &'static str,
    pub cancel: &'static str,
    pub share: &'static str,
    pub share_screen: &'static str,
    pub remember_selection: &'static str,
    pub window: &'static str,
    pub display: &'static str,
    pub untitled_window: &'static str,
    pub protected_window: &'static str,
    pub hidden_from_screen_share: &'static str,
    pub share_description: fn(Option<&str>) -> String,
}

struct Catalog {
    tags: &'static [&'static str],
    messages: &'static Messages,
}

static CATALOGS: &[Catalog] = &[
    Catalog {
        tags: &["en", "en-us"],
        messages: &en_us::MESSAGES,
    },
    Catalog {
        tags: &["zh-cn", "zh-sg", "zh-hans", "zh-hans-cn", "zh-hans-sg"],
        messages: &zh_cn::MESSAGES,
    },
];

pub fn messages_from_env() -> &'static Messages {
    messages_from_lookup(|name| env::var(name).ok())
}

fn messages_from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> &'static Messages {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| lookup(name).filter(|value| !value.trim().is_empty()));

    locale
        .as_deref()
        .and_then(messages_for_locale)
        .unwrap_or(&en_us::MESSAGES)
}

fn messages_for_locale(locale: &str) -> Option<&'static Messages> {
    let mut tag = normalize_locale(locale);
    if tag == "c" || tag == "posix" {
        return Some(&en_us::MESSAGES);
    }

    loop {
        if let Some(catalog) = CATALOGS
            .iter()
            .find(|catalog| catalog.tags.contains(&tag.as_str()))
        {
            return Some(catalog.messages);
        }

        let index = tag.rfind('-')?;
        tag.truncate(index);
    }
}

fn normalize_locale(locale: &str) -> String {
    locale
        .trim()
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_posix_locale_names() {
        assert_eq!(normalize_locale("zh_CN.UTF-8"), "zh-cn");
        assert_eq!(normalize_locale("zh_Hans_CN@variant"), "zh-hans-cn");
        assert_eq!(normalize_locale(" en_US.utf8 "), "en-us");
    }

    #[test]
    fn resolves_supported_locales_without_crossing_scripts() {
        assert_eq!(messages_for_locale("zh_CN.UTF-8").unwrap().locale, "zh-CN");
        assert_eq!(messages_for_locale("zh-Hans-CN").unwrap().locale, "zh-CN");
        assert_eq!(messages_for_locale("en_GB.UTF-8").unwrap().locale, "en-US");
        assert!(messages_for_locale("zh_TW.UTF-8").is_none());
        assert!(messages_for_locale("zh-Hant").is_none());
    }

    #[test]
    fn locale_categories_follow_posix_precedence() {
        let messages = messages_from_lookup(|name| match name {
            "LC_ALL" => Some(String::new()),
            "LC_MESSAGES" => Some(String::from("zh_CN.UTF-8")),
            "LANG" => Some(String::from("en_US.UTF-8")),
            _ => None,
        });
        assert_eq!(messages.locale, "zh-CN");

        let messages = messages_from_lookup(|name| match name {
            "LC_ALL" => Some(String::from("C")),
            "LC_MESSAGES" => Some(String::from("zh_CN.UTF-8")),
            _ => None,
        });
        assert_eq!(messages.locale, "en-US");
    }

    #[test]
    fn unsupported_locales_fall_back_to_english() {
        let messages =
            messages_from_lookup(|name| (name == "LANG").then(|| String::from("fr_FR.UTF-8")));
        assert_eq!(messages.locale, "en-US");
    }
}
