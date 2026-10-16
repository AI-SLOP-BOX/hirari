//! Display language: OS locale detection and the EN/JA string catalog.
//!
//! The UI was English-only regardless of the machine's language because no
//! code ever read the OS locale and no translated string existed anywhere.
//! This module owns both halves: resolving which language to show, and the
//! translated strings themselves.
//!
//! Language resolution order: explicit user override (Settings, persisted in
//! `UiSettings.language` as `"en"` or `"ja"`), otherwise the OS locale
//! (`"system"`, the default), otherwise English. Only Japanese has a
//! catalog today; every other locale falls back to English rather than
//! showing a half-translated UI.

/// Language override values stored in settings. `"system"` follows the OS.
pub(crate) const LANGUAGE_SYSTEM: &str = "system";
pub(crate) const LANGUAGE_ENGLISH: &str = "en";
pub(crate) const LANGUAGE_JAPANESE: &str = "ja";

/// The settings default: follow the OS locale.
pub(crate) fn default_language() -> String {
    LANGUAGE_SYSTEM.to_owned()
}

/// Returns the OS locale's language subtag in lowercase (`"ja"`, `"en"`).
/// `HIRARI_LOCALE` overrides the OS for tests and troubleshooting.
pub(crate) fn system_language() -> String {
    if let Ok(forced) = std::env::var("HIRARI_LOCALE") {
        let forced = normalize_language(&forced);
        if !forced.is_empty() {
            return forced;
        }
    }
    sys_locale::get_locale()
        .map(|locale| normalize_language(&locale))
        .filter(|language| !language.is_empty())
        .unwrap_or_else(|| LANGUAGE_ENGLISH.to_owned())
}

/// Resolves the display language: the stored override wins, `"system"` (or
/// anything unrecognized) follows the OS, and anything without a catalog
/// falls back to English.
pub(crate) fn resolve_language(stored: &str) -> String {
    match stored.trim().to_lowercase().as_str() {
        LANGUAGE_ENGLISH => LANGUAGE_ENGLISH.to_owned(),
        LANGUAGE_JAPANESE => LANGUAGE_JAPANESE.to_owned(),
        _ => {
            let system = system_language();
            if system == LANGUAGE_JAPANESE {
                LANGUAGE_JAPANESE.to_owned()
            } else {
                LANGUAGE_ENGLISH.to_owned()
            }
        }
    }
}

/// `"ja-JP"` / `"ja_JP.UTF-8"` / `"JA"` all become `"ja"`.
fn normalize_language(locale: &str) -> String {
    locale
        .split([',', ';'])
        .next()
        .unwrap_or_default()
        .split(['-', '_', '.'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect()
}

/// One entry of the UI string catalog: the key is the binding name used
/// from Slint, English is the source of truth, Japanese is the translation.
pub(crate) struct UiString {
    pub key: &'static str,
    pub en: &'static str,
    pub ja: &'static str,
}

/// The full catalog. Every key rendered through `AppStrings` in Slint must
/// appear here exactly once; the completeness test below enforces that both
/// languages cover every key with non-empty text.
pub(crate) const UI_STRINGS: &[UiString] = &[
    // Menu bar destinations.
    UiString { key: "menu_project", en: "Project", ja: "プロジェクト" },
    UiString { key: "menu_automation", en: "Automation", ja: "オートメーション" },
    UiString { key: "menu_stacks", en: "Stacks", ja: "スタック" },
    UiString { key: "menu_prefs", en: "Prefs", ja: "設定" },
    UiString { key: "menu_diagnostics", en: "Diagnostics", ja: "診断" },
    // Home screen.
    UiString { key: "home_title", en: "Start a session", ja: "セッションを始める" },
    UiString { key: "home_subtitle", en: "Create a working session or open a saved project.", ja: "作業セッションを作成するか、保存済みプロジェクトを開きます。" },
    UiString { key: "home_new_title", en: "New electronic session", ja: "新規エレクトロニックセッション" },
    UiString { key: "home_new_body", en: "Start with editable instrument and rhythm tracks.", ja: "編集可能な楽器・リズムトラックから始めます。" },
    UiString { key: "home_new_button", en: "CREATE SESSION", ja: "セッション作成" },
    UiString { key: "home_open_title", en: "Open a saved project", ja: "保存済みプロジェクトを開く" },
    UiString { key: "home_open_body", en: "Choose a Hirari project from your files or open one from Recent Projects.", ja: "ファイルから Hirari プロジェクトを選ぶか、最近のプロジェクトから開きます。" },
    UiString { key: "home_open_button", en: "BROWSE PROJECTS", ja: "プロジェクトを開く" },
    UiString { key: "home_recent", en: "Recent Projects", ja: "最近のプロジェクト" },
    UiString { key: "home_open_row", en: "OPEN", ja: "開く" },
    UiString { key: "home_templates", en: "Templates", ja: "テンプレート" },
    UiString { key: "home_templates_count", en: "4 project setups", ja: "プロジェクト構成4件" },
    UiString { key: "home_no_recent", en: "No recent projects yet. Open or save a project and it will appear here.", ja: "最近のプロジェクトはまだありません。プロジェクトを開くか保存するとここに表示されます。" },
    UiString { key: "home_saved_prefix", en: "", ja: "保存済みプロジェクト" },
    UiString { key: "home_saved_suffix", en: " saved projects", ja: "件" },
    // MixConsole.
    UiString { key: "mixer_title", en: "MixConsole", ja: "ミックスコンソール" },
    UiString { key: "mixer_channels", en: "CHANNELS", ja: "チャンネル" },
    UiString { key: "mixer_no_channels", en: "No channels yet · create a track to mix it here", ja: "チャンネルがありません · トラックを作成するとここに表示されます" },
    UiString { key: "mixer_inspector", en: "Channel", ja: "チャンネル" },
    UiString { key: "mixer_no_selection", en: "No channel selected", ja: "チャンネルが選択されていません" },
    UiString { key: "mixer_no_selection_body", en: "Pick a channel strip to inspect its volume, pan and inserts.", ja: "チャンネルストリップを選ぶと音量・パン・インサートを表示します。" },
    // Arrangement.
    UiString { key: "arrange_empty_lane", en: "Empty · import audio, or draw notes in the MIDI editor", ja: "空 · オーディオを読み込むか、MIDIエディタでノートを描きます" },
    // Diagnostics page.
    UiString { key: "diag_title", en: "DIAGNOSTIC PANEL", ja: "診断パネル" },
    UiString { key: "diag_subtitle", en: "Audio and plugin health", ja: "オーディオとプラグインの状態" },
    UiString { key: "diag_no_audio", en: "No audio output", ja: "音が出ません" },
    UiString { key: "diag_no_audio_body", en: "The engine is running but no audio device is connected, so playback is silent. Transport controls stay enabled for editing.", ja: "エンジンは動作していますが、オーディオデバイスが接続されていないため無音です。編集のためトランスポート操作は有効のままです。" },
    UiString { key: "diag_no_audio_steps", en: "1. Connect or select an output device in Audio Settings.  2. Reconnect the driver.  3. Restart the app if the device still does not appear.", ja: "1. オーディオ設定で出力デバイスを接続・選択します。  2. ドライバーを再接続します。  3. それでも表示されない場合はアプリを再起動します。" },
    UiString { key: "diag_criteria", en: "Criteria: no dropouts, no callback overruns, no leaked workers, no project corruption", ja: "基準: ドロップアウトなし・コールバック超過なし・ワーカー漏れなし・プロジェクト破損なし" },
    // Session recovery prompt.
    UiString { key: "recovery_title", en: "An unsaved Hirari session is available", ja: "未保存の Hirari セッションがあります" },
    UiString { key: "recovery_body", en: "Choose whether to restore it, discard its recovery files, or leave it for later.", ja: "復元するか、復旧ファイルを破棄するか、後で判断するか選んでください。" },
    UiString { key: "recovery_recover", en: "Recover session", ja: "セッションを復元" },
    UiString { key: "recovery_discard", en: "Discard recovery", ja: "復旧ファイルを破棄" },
    UiString { key: "recovery_later", en: "Later", ja: "後で" },
    // Settings: language selector.
    UiString { key: "settings_language", en: "Language", ja: "言語" },
    UiString { key: "settings_language_system", en: "System", ja: "システム" },
    UiString { key: "settings_language_english", en: "English", ja: "英語" },
    UiString { key: "settings_language_japanese", en: "Japanese", ja: "日本語" },
    UiString { key: "settings_language_note", en: "Takes effect on next launch.", ja: "次回起動時に反映されます。" },
    UiString { key: "settings_theme", en: "Appearance", ja: "外観" },
    UiString { key: "settings_theme_system", en: "System", ja: "システム" },
    UiString { key: "settings_theme_dark", en: "Dark", ja: "ダーク" },
    UiString { key: "settings_theme_light", en: "Light", ja: "ライト" },
];

/// Looks up one key for a resolved language (`"en"`/`"ja"`).
pub(crate) fn lookup(language: &str, key: &str) -> &'static str {
    UI_STRINGS
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| {
            if language == LANGUAGE_JAPANESE {
                entry.ja
            } else {
                entry.en
            }
        })
        .unwrap_or_default()
}

/// Installs the display language on the Slint `Tr` global. The resolved
/// language is moved into the lookup closure, so every binding evaluates
/// against it from the first frame. Called once at startup, before the UI
/// is shown; changing language afterwards requires a restart, which the
/// Settings screen says plainly.
pub(crate) fn install(ui: &crate::slint_ui::AppWindow, language: &str) {
    use slint::ComponentHandle;
    let language = language.to_owned();
    ui.global::<crate::slint_ui::Tr>()
        .on_tr(move |key| lookup(&language, key.as_str()).into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tags_normalize_to_a_language_subtag() {
        assert_eq!(normalize_language("ja-JP"), "ja");
        assert_eq!(normalize_language("ja_JP.UTF-8"), "ja");
        assert_eq!(normalize_language("JA"), "ja");
        assert_eq!(normalize_language("en-US"), "en");
        assert_eq!(normalize_language(""), "");
        assert_eq!(normalize_language("   "), "");
    }

    #[test]
    fn stored_override_wins_over_the_os_locale() {
        assert_eq!(resolve_language("en"), "en");
        assert_eq!(resolve_language("ja"), "ja");
        assert_eq!(resolve_language("JA"), "ja");
        assert_eq!(resolve_language("  en  "), "en");
    }

    #[test]
    fn unknown_overrides_follow_the_os_without_crashing() {
        // Whatever the machine reports, the answer is one of the two
        // supported languages — never the raw stored value.
        let resolved = resolve_language("xx");
        assert!(resolved == "en" || resolved == "ja", "got {resolved}");
        let system = resolve_language("system");
        assert!(system == "en" || system == "ja", "got {system}");
    }

    #[test]
    fn every_catalog_key_has_both_languages() {
        let mut keys = std::collections::HashSet::new();
        for entry in UI_STRINGS {
            assert!(!entry.key.is_empty(), "catalog key must not be empty");
            // Count fragments (`*_prefix`/`*_suffix`) are composed around a
            // number in Slint, so one side is legitimately empty: "" + 3 +
            // " saved projects" in English, "保存済みプロジェクト" + 3 +
            // "件" in Japanese. Every other entry must read as a complete
            // string in both languages.
            let is_fragment = entry.key.ends_with("_prefix") || entry.key.ends_with("_suffix");
            if !is_fragment {
                assert!(!entry.en.is_empty(), "{} must have English text", entry.key);
                assert!(
                    !entry.ja.is_empty(),
                    "{} must have Japanese text",
                    entry.key
                );
            } else {
                assert!(
                    !entry.en.is_empty() || !entry.ja.is_empty(),
                    "{} must not be empty in both languages",
                    entry.key
                );
            }
            assert_ne!(
                entry.en,
                entry.ja,
                "{key} has identical text in both languages; a copy-paste, not a translation",
                key = entry.key
            );
            assert!(
                keys.insert(entry.key),
                "duplicate catalog key {}",
                entry.key
            );
        }
        assert!(!UI_STRINGS.is_empty());
    }

    #[test]
    fn lookup_falls_back_to_english_for_unknown_keys_and_languages() {
        assert_eq!(lookup("ja", "home_title"), "セッションを始める");
        assert_eq!(lookup("en", "home_title"), "Start a session");
        assert_eq!(lookup("fr", "home_title"), "Start a session");
        assert_eq!(lookup("ja", "no-such-key"), "");
    }

    #[test]
    fn catalog_keys_match_the_keys_used_in_slint() {
        // Every `Tr.tr("key")` literal in the Slint sources must exist in the
        // Rust catalog (a typo fails the Slint compile too, but this names
        // the contract), and every catalog key must be used somewhere (dead
        // translations rot silently). Both sides are scanned, so adding a
        // string in one place without the other fails loudly. Runs from the
        // package root, which is the test working directory.
        fn collect_slint_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_slint_files(&path, out);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("slint") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        collect_slint_files(std::path::Path::new("ui"), &mut files);
        assert!(!files.is_empty(), "Slint sources must be discoverable");
        let mut used = std::collections::HashSet::new();
        for path in &files {
            let source = std::fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("Slint source must be readable: {}", path.display()));
            for line in source.lines() {
                // Example keys in `//` comments must not count as uses.
                let code = line.split("//").next().unwrap_or_default();
                let mut rest = code;
                while let Some(start) = rest.find("Tr.tr(\"") {
                    rest = &rest[start + 7..];
                    if let Some(end) = rest.find('"') {
                        used.insert(rest[..end].to_owned());
                        rest = &rest[end + 1..];
                    } else {
                        break;
                    }
                }
            }
        }
        let catalog: std::collections::HashSet<String> = UI_STRINGS
            .iter()
            .map(|entry| entry.key.to_owned())
            .collect();
        let missing: Vec<_> = used.difference(&catalog).collect();
        assert!(
            missing.is_empty(),
            "Slint uses keys missing from the Rust catalog: {missing:?}"
        );
        let dead: Vec<_> = catalog.difference(&used).collect();
        assert!(
            dead.is_empty(),
            "catalog keys never used from Slint: {dead:?}"
        );
    }
}
