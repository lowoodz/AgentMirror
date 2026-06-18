//! Localized copy for rule-based reflection reports (non-LLM baseline).

use crate::critic::{CriticInput, TaskKind};
use crate::locale::ReportLanguage;
use crate::models::{CriticsAnalysis, CriticsScore, Suggestion};

pub fn no_clear_goal_issue(language: ReportLanguage) -> String {
    match language {
        ReportLanguage::Zh => "对话中未检测到明确目标".to_string(),
        ReportLanguage::En => "No clear goal detected in conversation".to_string(),
    }
}

pub fn no_verification_issue(language: ReportLanguage) -> String {
    match language {
        ReportLanguage::Zh => "实现类操作后未检测到验证步骤".to_string(),
        ReportLanguage::En => {
            "No verification step detected after implementation actions".to_string()
        }
    }
}

pub fn add_verification_suggestion(language: ReportLanguage) -> Suggestion {
    match language {
        ReportLanguage::Zh => Suggestion {
            message: "在标记任务完成前增加测试或校验步骤".to_string(),
            rationale: "修复与编码类任务需要显式验证".to_string(),
            priority: "high".to_string(),
        },
        ReportLanguage::En => Suggestion {
            message: "Add a test or validation step before marking the task complete".to_string(),
            rationale: "Bugfix and coding tasks benefit from explicit verification".to_string(),
            priority: "high".to_string(),
        },
    }
}

pub fn summarize_findings_suggestion(language: ReportLanguage) -> Suggestion {
    match language {
        ReportLanguage::Zh => Suggestion {
            message: "请针对原始目标给出带明确结论的总结".to_string(),
            rationale: "多步智能体任务以清晰结果收尾更便于复盘".to_string(),
            priority: "medium".to_string(),
        },
        ReportLanguage::En => Suggestion {
            message: "Summarize findings with an explicit conclusion for the original goal"
                .to_string(),
            rationale: "Multi-step agent runs are easier to review when they end with a clear outcome"
                .to_string(),
            priority: "medium".to_string(),
        },
    }
}

pub fn redundant_actions_issue(language: ReportLanguage) -> String {
    match language {
        ReportLanguage::Zh => "检测到重复相似操作，可能存在冗余".to_string(),
        ReportLanguage::En => "Repeated similar actions detected — possible redundancy".to_string(),
    }
}

pub fn break_into_subgoals_suggestion(language: ReportLanguage, turn_count: u32) -> Suggestion {
    match language {
        ReportLanguage::Zh => Suggestion {
            message: "考虑将任务拆分为更小的子目标".to_string(),
            rationale: format!("本任务共使用 {} 轮 LLM 对话", turn_count),
            priority: "medium".to_string(),
        },
        ReportLanguage::En => Suggestion {
            message: "Consider breaking the task into smaller sub-goals".to_string(),
            rationale: format!("Run used {turn_count} LLM turns"),
            priority: "medium".to_string(),
        },
    }
}

pub fn destructive_shell_issue(language: ReportLanguage) -> String {
    match language {
        ReportLanguage::Zh => "检测到潜在破坏性 shell 操作".to_string(),
        ReportLanguage::En => "Potentially destructive shell action detected".to_string(),
    }
}

pub fn build_analyses(
    language: ReportLanguage,
    score: &CriticsScore,
    input: CriticInput<'_>,
    task_kind: TaskKind,
    has_goal: bool,
    has_result: bool,
    has_verify: bool,
    has_observation: bool,
    action_count: usize,
    unique_action_count: usize,
) -> CriticsAnalysis {
    let goal_snip = truncate_chars(input.goal, 120);

    let alignment = if has_goal {
        match language {
            ReportLanguage::Zh => format!(
                "已记录目标（「{}」）。共 {} 次操作、{} 轮对话，请审视各步骤是否仍服务于该目标 — 得分 {} 表示{}对齐。",
                goal_snip,
                action_count,
                input.turn_count,
                score.alignment,
                alignment_label(language, score.alignment)
            ),
            ReportLanguage::En => format!(
                "A goal was recorded (\"{}\"). With {} action(s) across {} turn(s), review whether each step still serves this objective and context — score {} suggests {} alignment.",
                goal_snip,
                action_count,
                input.turn_count,
                score.alignment,
                alignment_label(language, score.alignment)
            ),
        }
    } else {
        match language {
            ReportLanguage::Zh => {
                "未能从轨迹中提取明确目标，难以判断操作是否符合意图；智能体可能偏离或会话目标未被捕获。"
                    .to_string()
            }
            ReportLanguage::En => {
                "No clear goal was extracted from the trace, so actions cannot be reliably judged against stated intent. The agent may be drifting or the session goal was never captured.".to_string()
            }
        }
    };

    let necessity = if score.necessity < 60 {
        match language {
            ReportLanguage::Zh => format!(
                "共 {} 次操作但仅 {} 种不同模式 — 可能存在重复步骤。得分 {} 表示冗余偏高。",
                action_count, unique_action_count, score.necessity
            ),
            ReportLanguage::En => format!(
                "Detected {} total actions but only {} distinct action patterns — likely redundant or repeated steps. Score {} indicates unnecessary repetition that could be trimmed.",
                action_count, unique_action_count, score.necessity
            ),
        }
    } else if action_count == 0 {
        match language {
            ReportLanguage::Zh => "未记录工具操作；在智能体开始执行前必要性暂不适用。".to_string(),
            ReportLanguage::En => {
                "No tool actions were recorded; necessity is moot until the agent executes steps toward the goal.".to_string()
            }
        }
    } else {
        match language {
            ReportLanguage::Zh => format!(
                "记录 {} 次操作、{} 种模式 — 未发现明显冗余（得分 {}），各步骤对当前轨迹较为必要。",
                action_count, unique_action_count, score.necessity
            ),
            ReportLanguage::En => format!(
                "Recorded {} action(s) with {} distinct patterns — no major redundancy detected (score {}). Each step appears reasonably necessary for the current trajectory.",
                action_count, unique_action_count, score.necessity
            ),
        }
    };

    let completeness = match task_kind {
        TaskKind::Explore => {
            if has_result {
                match language {
                    ReportLanguage::Zh => format!(
                        "探索类任务已有明确结果/结论（得分 {}），调查与归纳较完整。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Exploration run ended with an explicit result/conclusion (score {}). The approach appears to cover investigation and synthesis for the goal.",
                        score.completeness
                    ),
                }
            } else if has_observation && action_count > 0 {
                match language {
                    ReportLanguage::Zh => format!(
                        "智能体通过 {} 次操作收集观察，但未提取到最终结果（得分 {}）；可能缺少分析与明确答复。",
                        action_count, score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "The agent gathered observations via {} action(s) but no final result/conclusion was extracted (score {}). Consider whether analysis and a definitive answer to the goal are missing.",
                        action_count, score.completeness
                    ),
                }
            } else {
                match language {
                    ReportLanguage::Zh => format!(
                        "探索可能不完整 — 相对目标观察或操作偏少（得分 {}），关键调查阶段可能缺失。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Exploration appears incomplete — few observations or actions relative to the goal (score {}). Key investigation phases may be missing.",
                        score.completeness
                    ),
                }
            }
        }
        TaskKind::Coding => {
            if has_verify {
                match language {
                    ReportLanguage::Zh => format!(
                        "实现后包含验证步骤（得分 {}），编码流程在收尾前有校验。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Implementation was followed by a verification step (score {}). The coding workflow includes validation before closure.",
                        score.completeness
                    ),
                }
            } else if action_count >= 2 {
                match language {
                    ReportLanguage::Zh => format!(
                        "有实现类操作但未检测到验证/测试步骤（得分 {}），缺少校验可能使修复不完整。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Implementation actions were recorded but no verification/test step was detected (score {}). The fix may be incomplete without validation.",
                        score.completeness
                    ),
                }
            } else {
                match language {
                    ReportLanguage::Zh => format!(
                        "编码类任务操作证据有限（得分 {}），规划、实现、验证阶段可能未齐备。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Coding task with limited action evidence (score {}). Plan, implement, and verify phases may not all be present.",
                        score.completeness
                    ),
                }
            }
        }
        TaskKind::Chat => {
            if has_result {
                match language {
                    ReportLanguage::Zh => format!(
                        "对话已达到明确结果（得分 {}），似乎已回应用户请求。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Conversation reached a stated outcome (score {}). The dialogue appears to resolve the user's request.",
                        score.completeness
                    ),
                }
            } else {
                match language {
                    ReportLanguage::Zh => format!(
                        "多轮对话未提取到清晰结果（得分 {}），回复可能不完整或仍在进行。",
                        score.completeness
                    ),
                    ReportLanguage::En => format!(
                        "Multi-turn chat without a clear extracted result (score {}). The response may be partial or still in progress.",
                        score.completeness
                    ),
                }
            }
        }
    };

    let efficiency = match input.turn_count {
        0..=5 => match language {
            ReportLanguage::Zh => format!(
                "使用 {} 轮对话 — 执行较紧凑（得分 {}），路径长度合理。",
                input.turn_count, score.efficiency
            ),
            ReportLanguage::En => format!(
                "Used {} LLM turn(s) — compact execution (score {}). Path length looks reasonable for the scope.",
                input.turn_count, score.efficiency
            ),
        },
        6..=15 => match language {
            ReportLanguage::Zh => format!(
                "消耗 {} 轮（得分 {}）；注意绕路，范围可能仍可接受。",
                input.turn_count, score.efficiency
            ),
            ReportLanguage::En => format!(
                "{} turns consumed (score {}). Monitor for detours; scope may still be acceptable.",
                input.turn_count, score.efficiency
            ),
        },
        16..=30 => match language {
            ReportLanguage::Zh => format!(
                "{} 轮偏多（得分 {}）；可能绕路或频繁重规划 — 可考虑子目标或更紧提示。",
                input.turn_count, score.efficiency
            ),
            ReportLanguage::En => format!(
                "{} turns is relatively heavy (score {}). The agent may be taking indirect routes or re-planning often — consider sub-goals or tighter prompts.",
                input.turn_count, score.efficiency
            ),
        },
        _ => match language {
            ReportLanguage::Zh => format!(
                "{} 轮表明路径较长、效率偏低（得分 {}）；拆分为更小任务更易复盘并节省成本。",
                input.turn_count, score.efficiency
            ),
            ReportLanguage::En => format!(
                "{} turns indicates a long, potentially inefficient path (score {}). Breaking the task into smaller runs would improve reviewability and cost.",
                input.turn_count, score.efficiency
            ),
        },
    };

    let mut safety_parts: Vec<String> = Vec::new();
    if input.safety_findings.is_empty() {
        safety_parts.push(match language {
            ReportLanguage::Zh => "本任务未标记策略或 DLP 安全问题。".to_string(),
            ReportLanguage::En => {
                "No policy or DLP safety findings were flagged for this run.".to_string()
            }
        });
    } else {
        safety_parts.push(match language {
            ReportLanguage::Zh => format!(
                "策略/DLP 标记 {} 项：{}。",
                input.safety_findings.len(),
                input.safety_findings.join("；")
            ),
            ReportLanguage::En => format!(
                "Policy/DLP flagged {} issue(s): {}.",
                input.safety_findings.len(),
                input.safety_findings.join("; ")
            ),
        });
    }
    if score.safety < 70 {
        safety_parts.push(match language {
            ReportLanguage::Zh => {
                "检测到潜在高风险工具使用（破坏性命令或敏感操作）— 回放前请审阅操作摘要。".to_string()
            }
            ReportLanguage::En => {
                "Potentially risky tool usage (destructive commands or sensitive operations) was detected — review action summaries before replay.".to_string()
            }
        });
    } else {
        safety_parts.push(match language {
            ReportLanguage::Zh => format!(
                "总体安全得分 {} — 除常规智能体工具外未见高风险模式。",
                score.safety
            ),
            ReportLanguage::En => format!(
                "Overall safety score {} — no high-risk patterns beyond routine agent tooling.",
                score.safety
            ),
        });
    }

    CriticsAnalysis {
        alignment,
        necessity,
        completeness,
        efficiency,
        safety: safety_parts.join(" "),
    }
}

fn alignment_label(language: ReportLanguage, score: u8) -> &'static str {
    match (language, score) {
        (ReportLanguage::Zh, 0..=49) => "较弱",
        (ReportLanguage::Zh, 50..=69) => "中等",
        (ReportLanguage::Zh, 70..=84) => "良好",
        (ReportLanguage::Zh, _) => "较强",
        (ReportLanguage::En, 0..=49) => "weak",
        (ReportLanguage::En, 50..=69) => "moderate",
        (ReportLanguage::En, 70..=84) => "good",
        (ReportLanguage::En, _) => "strong",
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
