#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReportLanguage {
    #[default]
    En,
    Zh,
}

impl ReportLanguage {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "chinese" => Self::Zh,
            _ => Self::En,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }

    pub fn write_instruction(self) -> &'static str {
        match self {
            Self::Zh => "Write ALL narrative and analysis fields in Chinese (简体中文).",
            Self::En => "Write ALL narrative and analysis fields in English.",
        }
    }

    pub fn daily_all_agents_label(self) -> &'static str {
        match self {
            Self::Zh => "全部智能体",
            Self::En => "All agents",
        }
    }
}
