mod english;
mod french;
mod german;
mod japanese;
mod spanish;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageId {
    English,
    Spanish,
    French,
    German,
    Japanese,
}

#[allow(dead_code)]
impl LanguageId {
    pub const ALL: [LanguageId; 5] = [
        LanguageId::English,
        LanguageId::Spanish,
        LanguageId::French,
        LanguageId::German,
        LanguageId::Japanese,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
            Self::Japanese => "ja",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Spanish => "Español",
            Self::French => "Français",
            Self::German => "Deutsch",
            Self::Japanese => "日本語",
        }
    }

    pub fn strings(self) -> Strings {
        match self {
            Self::English => english::STRINGS,
            Self::Spanish => spanish::STRINGS,
            Self::French => french::STRINGS,
            Self::German => german::STRINGS,
            Self::Japanese => japanese::STRINGS,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let normalized = code.trim().replace('_', "-").to_ascii_lowercase();
        if normalized.is_empty() || normalized == "system" {
            return None;
        }

        match normalized.split('-').next().unwrap_or_default() {
            "en" => Some(Self::English),
            "es" => Some(Self::Spanish),
            "fr" => Some(Self::French),
            "de" => Some(Self::German),
            "ja" => Some(Self::Japanese),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct Strings {
    pub window_title: &'static str,
    pub refresh: &'static str,
    pub update_frequency: &'static str,
    pub one_minute: &'static str,
    pub five_minutes: &'static str,
    pub fifteen_minutes: &'static str,
    pub one_hour: &'static str,
    pub settings: &'static str,
    pub start_at_login: &'static str,
    pub language: &'static str,
    pub system_default: &'static str,
    pub check_for_updates: &'static str,
    pub checking_for_updates: &'static str,
    pub updates: &'static str,
    pub update_in_progress: &'static str,
    pub up_to_date: &'static str,
    pub up_to_date_short: &'static str,
    pub update_failed: &'static str,
    pub applying_update: &'static str,
    pub update_to: &'static str,
    pub update_available: &'static str,
    pub update_prompt_now: &'static str,
    pub exit: &'static str,
    pub session_window: &'static str,
    pub weekly_window: &'static str,
    pub now: &'static str,
    pub day_suffix: &'static str,
    pub hour_suffix: &'static str,
    pub minute_suffix: &'static str,
    pub second_suffix: &'static str,
}

pub fn resolve_language(language_override: Option<LanguageId>) -> LanguageId {
    language_override.unwrap_or_else(detect_system_language)
}

pub fn detect_system_language() -> LanguageId {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find_map(|locale| LanguageId::from_code(&locale))
        .unwrap_or(LanguageId::English)
}
