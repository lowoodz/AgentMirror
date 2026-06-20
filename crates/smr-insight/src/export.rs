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
         .overview-panel {{ display:flex; flex-direction:column; gap:10px; padding:10px 12px; border-radius:8px; background:var(--inset); border:1px solid var(--border); }}\
         .overview-stats {{ display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:8px; }}\
         .overview-stat {{ display:flex; flex-direction:row; align-items:center; gap:8px; padding:8px 10px; border-radius:8px; background:var(--surface); border:1px solid var(--border); min-width:0; }}\
         .overview-emoji {{ font-size:18px; line-height:1; flex-shrink:0; align-self:center; }}\
         .overview-stat-body {{ display:flex; flex-direction:column; justify-content:center; gap:2px; min-width:0; flex:1; }}\
         .overview-val {{ font-size:18px; font-weight:700; line-height:1.1; }}\
         .overview-lbl {{ font-size:10px; color:var(--muted); line-height:1.25; }}\
         .overview-bar {{ display:flex; width:100%; height:8px; border-radius:999px; overflow:hidden; border:1px solid var(--border); background:#e2e8f0; }}\
         .overview-bar-seg {{ height:100%; min-width:0; }}\
         .overview-bar-seg.done {{ background:linear-gradient(90deg,#22c55e,#16a34a); }}\
         .overview-bar-seg.run {{ background:linear-gradient(90deg,#38bdf8,#0284c7); }}\
         .overview-bar-seg.fail {{ background:linear-gradient(90deg,#f87171,#dc2626); }}\
         .overview-bar-seg.empty {{ flex:1; background:#cbd5e1; }}\
         .overview-bar-head {{ display:flex; align-items:center; gap:6px; margin-bottom:6px; font-size:11px; font-weight:600; color:var(--muted); }}\
         .overview-legend {{ display:flex; flex-wrap:wrap; gap:6px 12px; margin-top:6px; font-size:11px; color:var(--muted); }}\
         .overview-legend span {{ white-space:nowrap; }}\
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
    body.push_str(&render_daily_overview_html(rep, zh));

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

fn render_daily_overview_html(rep: &DailyReport, zh: bool) -> String {
    let done = rep.runs_completed;
    let running = rep.runs_running;
    let failed = rep.runs_failed;
    let total = done + running + failed;
    let bar_total = total.max(1);
    let pct = |n: u32| {
        if total == 0 {
            0
        } else {
            ((n as f64 / bar_total as f64) * 100.0).round().max(2.0) as u32
        }
    };
    let task_lbl = if zh { "任务" } else { "Tasks" };
    let agents_lbl = if zh { "活跃 Agent" } else { "Active agents" };
    let turns_lbl = if zh { "总 LLM 轮次" } else { "Total LLM turns" };
    let tokens_lbl = if zh { "Token 消耗" } else { "Token usage" };
    let done_lbl = if zh {
        format!("完成 {done}")
    } else {
        format!("Done {done}")
    };
    let run_lbl = if zh {
        format!("进行中 {running}")
    } else {
        format!("Running {running}")
    };
    let fail_lbl = if zh {
        format!("失败 {failed}")
    } else {
        format!("Failed {failed}")
    };
    let bar_segs = if total == 0 {
        r#"<div class="overview-bar-seg empty"></div>"#.to_string()
    } else {
        let mut segs = String::new();
        if done > 0 {
            segs.push_str(&format!(
                r#"<div class="overview-bar-seg done" style="width:{}%"></div>"#,
                pct(done)
            ));
        }
        if running > 0 {
            segs.push_str(&format!(
                r#"<div class="overview-bar-seg run" style="width:{}%"></div>"#,
                pct(running)
            ));
        }
        if failed > 0 {
            segs.push_str(&format!(
                r#"<div class="overview-bar-seg fail" style="width:{}%"></div>"#,
                pct(failed)
            ));
        }
        segs
    };
    format!(
        r#"</h2><div class="overview-panel"><div class="overview-stats">
        <div class="overview-stat"><span class="overview-emoji">🤖</span><div class="overview-stat-body"><span class="overview-val">{active}</span><span class="overview-lbl">{agents_lbl}</span></div></div>
        <div class="overview-stat"><span class="overview-emoji">📋</span><div class="overview-stat-body"><span class="overview-val">{total}</span><span class="overview-lbl">{task_lbl}</span></div></div>
        <div class="overview-stat"><span class="overview-emoji">💬</span><div class="overview-stat-body"><span class="overview-val">{turns}</span><span class="overview-lbl">{turns_lbl}</span></div></div>
        <div class="overview-stat"><span class="overview-emoji">🔢</span><div class="overview-stat-body"><span class="overview-val">{tokens}</span><span class="overview-lbl">{tokens_lbl}</span></div></div>
        </div><div class="overview-bar-head"><span>📊</span><span>{task_lbl}</span></div>
        <div class="overview-bar">{bar_segs}</div>
        <div class="overview-legend"><span>✅ {done_lbl}</span><span>⏳ {run_lbl}</span><span>❌ {fail_lbl}</span></div>
        </div></section>"#,
        active = rep.active_agents,
        turns = rep.total_turns,
        tokens = rep.total_tokens,
        agents_lbl = html_escape(agents_lbl),
        task_lbl = html_escape(task_lbl),
        turns_lbl = html_escape(turns_lbl),
        tokens_lbl = html_escape(tokens_lbl),
        done_lbl = html_escape(&done_lbl),
        run_lbl = html_escape(&run_lbl),
        fail_lbl = html_escape(&fail_lbl),
        bar_segs = bar_segs,
    )
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
