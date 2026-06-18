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

    pub fn empty_execution_summary(self) -> &'static str {
        match self {
            Self::Zh => "尚无已记录的操作",
            Self::En => "No actions recorded yet",
        }
    }

    pub fn daily_baseline_summary(
        self,
        run_count: usize,
        date: chrono::NaiveDate,
        completed: u32,
        running: u32,
        failed: u32,
        total_turns: u32,
    ) -> String {
        match self {
            Self::Zh => format!(
                "{} 日共 {} 个任务 — 完成 {}、进行中 {}、失败 {}，合计 {} 轮 LLM 对话。",
                date, run_count, completed, running, failed, total_turns
            ),
            Self::En => format!(
                "{} runs on {} — {} completed, {} in progress, {} failed, {} LLM turns total.",
                run_count, date, completed, running, failed, total_turns
            ),
        }
    }

    pub fn daily_issue_run_line(self, run_short: &str, message: &str) -> String {
        match self {
            Self::Zh => format!("任务 #{}：{}", run_short, message),
            Self::En => format!("Run #{}: {}", run_short, message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn daily_baseline_summary_localized() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        let zh = ReportLanguage::Zh.daily_baseline_summary(3, date, 2, 1, 0, 12);
        assert!(zh.contains("2026-06-18"));
        assert!(zh.contains("完成"));
        let en = ReportLanguage::En.daily_baseline_summary(3, date, 2, 1, 0, 12);
        assert!(en.contains("completed"));
    }
}
