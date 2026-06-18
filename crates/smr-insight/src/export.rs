use crate::models::{DailyIssueItem, DailyReport, DailyTaskProgress};
use crate::report::{format_issue_score_suffix, format_task_progress_line, run_short_id, task_status_icon};

pub fn daily_reports_html(reports: &[DailyReport], title: &str) -> String {
    let body: String = reports.iter().map(render_daily_report_html).collect();
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"/>\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"/>\
         <title>{title}</title>\
         <style>\
         :root {{ --text:#0f172a; --muted:#64748b; --border:#e2e8f0; --surface:#fff; --inset:#f8fafc; \
           --green:#16a34a; --blue:#0284c7; --red:#dc2626; --amber:#d97706; }}\
         * {{ box-sizing:border-box; }}\
         body {{ font-family: ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif; \
           margin:0; padding:24px 28px 36px; color:var(--text); background:#f1f5f9; line-height:1.5; font-size:13px; }}\
         h1 {{ font-size:20px; font-weight:700; margin:0 0 16px; letter-spacing:-.02em; }}\
         .report {{ background:var(--surface); border:1px solid var(--border); border-radius:10px; \
           overflow:hidden; margin-bottom:16px; box-shadow:0 1px 2px rgba(15,23,42,.05); page-break-inside:avoid; }}\
         .report-header {{ display:flex; flex-direction:row; flex-wrap:nowrap; align-items:center; justify-content:space-between; gap:12px; height:36px; min-height:36px; padding:0 14px; border-bottom:1px solid var(--border); \
           background:linear-gradient(135deg,#eff6ff 0%,#f0fdf4 100%); min-width:0; box-sizing:border-box; }}\
         .report-title {{ margin:0; padding:0; flex:1; min-width:0; font-size:14px; font-weight:700; letter-spacing:-.01em; line-height:1.2; overflow:visible; white-space:normal; word-break:break-word; }}\
         .report-meta {{ flex-shrink:0; font-size:11px; color:var(--muted); line-height:1.2; text-align:right; white-space:nowrap; display:flex; align-items:center; justify-content:flex-end; gap:6px; }}\
         .badge {{ display:none; }}\
         .body {{ padding:14px 16px 16px; display:flex; flex-direction:column; gap:12px; }}\
         .section {{ min-width:0; }}\
         .section h2 {{ margin:0 0 6px; font-size:14px; font-weight:700; letter-spacing:0; text-transform:none; color:var(--text); }}\
         .section h3 {{ margin:0 0 6px; font-size:14px; font-weight:700; letter-spacing:0; text-transform:none; color:var(--text); }}\
         .daily-items {{ margin:0; padding:0; list-style:none; display:flex; flex-direction:column; gap:5px; font-size:13px; }}\
         .daily-items li {{ display:flex; gap:8px; align-items:baseline; line-height:1.5; min-width:0; }}\
         .daily-items li > span:last-child {{ flex:1; min-width:0; }}\
         .daily-marker {{ flex-shrink:0; width:14px; text-align:center; line-height:1.5; font-weight:700; }}\
         .daily-marker.dot {{ display:inline-flex; align-items:center; justify-content:center; min-height:1.5em; }}\
         .daily-marker.dot::before {{ content:''; width:7px; height:7px; border-radius:999px; display:block; box-sizing:border-box; }}\
         .daily-marker.dot.overview::before {{ background:#0284c7; border:1px solid #0369a1; }}\
         .daily-marker.dot.issues::before {{ background:#dc2626; border:1px solid #b91c1c; }}\
         .daily-marker.dot.tips::before {{ background:#16a34a; border:1px solid #15803d; }}\
         .daily-marker.icon.done {{ color:#16a34a; }}\
         .daily-marker.icon.fail {{ color:#dc2626; }}\
         .daily-marker.icon.run {{ color:#0284c7; }}\
         .run-tag {{ font-family:ui-monospace,monospace; font-size:11px; color:var(--blue); }}\
         .score {{ color:var(--muted); font-size:12px; }}\
         @media print {{ body {{ background:#fff; padding:10mm; }} .report {{ box-shadow:none; }} }}\
         </style></head><body>\
         <h1>{title}</h1>{body}</body></html>",
        title = html_escape(title),
    )
}

fn render_daily_report_html(rep: &DailyReport) -> String {
    let zh = rep.language() == crate::locale::ReportLanguage::Zh;
    let mode_label = if rep.llm_enhanced {
        if zh { "LLM" } else { "LLM" }
    } else if zh {
        "规则基线"
    } else {
        "Rule baseline"
    };
    let generated = rep
        .generated_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let title_suffix = if zh { " · 日报" } else { " · Daily" };
    let gen_label = if zh { "生成于" } else { "Generated" };
    let meta = format!("{gen_label} {generated} （{mode_label}）");

    let mut body = String::new();
    body.push_str(&format!(
        r#"<article class="report"><div class="report-header"><div class="report-title">{name}{suffix}</div>
        <div class="report-meta">{meta}</div></div><div class="body">"#,
        name = html_escape(&rep.display_name),
        suffix = title_suffix,
        meta = html_escape(&meta),
    ));

    body.push_str(r#"<section class="section"><h2>"#);
    body.push_str(if zh { "总览" } else { "Overview" });
    body.push_str(r#"</h2><ul class="daily-items overview">"#);
    body.push_str(&format!(
        "<li><span class=\"daily-marker dot overview\"></span><span>{}: {}</span></li>",
        if zh { "活跃 Agent" } else { "Active agents" },
        rep.active_agents
    ));
    body.push_str(&format!(
        "<li><span class=\"daily-marker dot overview\"></span><span>{}: {} / {}: {} / {}: {}</span></li>",
        if zh { "完成任务" } else { "Completed" },
        rep.runs_completed,
        if zh { "进行中" } else { "In progress" },
        rep.runs_running,
        if zh { "失败" } else { "Failed" },
        rep.runs_failed,
    ));
    body.push_str(&format!(
        "<li><span class=\"daily-marker dot overview\"></span><span>{}: {}</span></li></ul></section>",
        if zh { "总 LLM 轮次" } else { "Total LLM turns" },
        rep.total_turns,
    ));

    let tasks = daily_tasks(rep);
    if !tasks.is_empty() {
        body.push_str(r#"<section class="section"><h3>"#);
        body.push_str(if zh {
            "任务及进展"
        } else {
            "Tasks & progress"
        });
        body.push_str("</h3><ul class=\"daily-items tasks\">");
        for task in &tasks {
            let icon = task_status_icon(&task.status);
            let icon_cls = task_icon_class(&task.status);
            let line = format_task_progress_line(task, zh);
            body.push_str(&format!(
                "<li><span class=\"daily-marker icon {icon_cls}\">{icon}</span><span>{line}</span></li>",
                icon_cls = icon_cls,
                icon = icon,
                line = html_escape(&line),
            ));
        }
        body.push_str("</ul></section>");
    }

    let issues = daily_issues(rep);
    if !issues.is_empty() {
        body.push_str(r#"<section class="section"><h3>"#);
        body.push_str(if zh {
            "问题/风险"
        } else {
            "Issues & risks"
        });
        body.push_str("</h3><ul class=\"daily-items issues\">");
        for issue in &issues {
            body.push_str(&format!(
                "<li><span class=\"daily-marker dot issues\"></span><span><span class=\"run-tag\">Run #{}</span>: {msg}{score}</span></li>",
                html_escape(&run_short_id(&issue.run_id)),
                msg = html_escape(&issue.message),
                score = html_escape(&format_issue_score_suffix(issue, zh)),
            ));
        }
        body.push_str("</ul></section>");
    }

    if !rep.top_suggestions.is_empty() {
        body.push_str(r#"<section class="section"><h2>"#);
        body.push_str(if zh {
            "改进建议"
        } else {
            "Recommendations"
        });
        body.push_str("</h2><ul class=\"daily-items tips\">");
        for s in &rep.top_suggestions {
            body.push_str(&format!(
                "<li><span class=\"daily-marker dot tips\"></span><span>{}</span></li>",
                html_escape(s)
            ));
        }
        body.push_str("</ul></section>");
    }

    body.push_str("</div></article>");
    body
}

fn daily_tasks(rep: &DailyReport) -> Vec<DailyTaskProgress> {
    if !rep.task_progress.is_empty() {
        return rep.task_progress.clone();
    }
    rep.run_summaries
        .iter()
        .map(|r| DailyTaskProgress {
            run_id: r.run_id.clone(),
            goal: r.goal.clone(),
            status: r.status.clone(),
            turn_count: r.turn_count,
            duration_minutes: None,
        })
        .collect()
}

fn daily_issues(rep: &DailyReport) -> Vec<DailyIssueItem> {
    if !rep.daily_issues.is_empty() {
        return rep.daily_issues.clone();
    }
    rep.top_issues
        .iter()
        .map(|msg| DailyIssueItem {
            run_id: String::new(),
            message: msg.clone(),
            dimension: None,
            score: None,
        })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn task_icon_class(status: &str) -> &'static str {
    let s = status.to_ascii_lowercase();
    if s.contains("complete") || s.contains("done") || s.contains("success") {
        "done"
    } else if s.contains("fail") || s.contains("error") {
        "fail"
    } else {
        "run"
    }
}
