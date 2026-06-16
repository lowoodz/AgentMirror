use crate::models::DailyReport;

pub fn daily_reports_html(reports: &[DailyReport], title: &str) -> String {
    let body: String = reports.iter().map(render_daily_report_html).collect();
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"/>\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"/>\
         <title>{title}</title>\
         <style>\
         :root {{ --text:#0f172a; --muted:#64748b; --border:#e2e8f0; --surface:#fff; --inset:#f8fafc; \
           --green:#16a34a; --blue:#0284c7; --red:#dc2626; --amber:#d97706; --violet:#7c3aed; }}\
         * {{ box-sizing:border-box; }}\
         body {{ font-family: ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif; \
           margin:0; padding:28px 32px 40px; color:var(--text); background:#f1f5f9; line-height:1.5; }}\
         h1 {{ font-size:22px; font-weight:700; margin:0 0 20px; letter-spacing:-.02em; }}\
         .report {{ background:var(--surface); border:1px solid var(--border); border-radius:12px; \
           overflow:hidden; margin-bottom:20px; box-shadow:0 1px 2px rgba(15,23,42,.06); page-break-inside:avoid; }}\
         .report-header {{ display:flex; justify-content:space-between; gap:16px; padding:20px 22px 18px; \
           border-bottom:1px solid var(--border); background:linear-gradient(135deg,#eff6ff 0%,#f0fdf4 100%); }}\
         .eyebrow {{ margin:0 0 4px; font-size:10px; font-weight:700; letter-spacing:.08em; text-transform:uppercase; color:var(--muted); }}\
         .report-title {{ margin:0; font-size:20px; font-weight:700; letter-spacing:-.02em; }}\
         .report-sub {{ margin:4px 0 0; font-size:13px; color:var(--muted); }}\
         .meta-col {{ text-align:right; font-size:11px; color:var(--muted); }}\
         .badge {{ display:inline-block; font-size:10px; font-weight:600; padding:3px 10px; border-radius:999px; \
           border:1px solid var(--border); background:var(--inset); margin-bottom:6px; }}\
         .badge.llm {{ color:var(--green); border-color:#bbf7d0; background:#f0fdf4; }}\
         .body {{ padding:18px 22px 22px; display:flex; flex-direction:column; gap:16px; }}\
         .kpis {{ display:grid; grid-template-columns:repeat(4,1fr); gap:10px; }}\
         .kpi {{ padding:12px 14px; border:1px solid var(--border); border-radius:8px; background:var(--inset); }}\
         .kpi-val {{ display:block; font-size:22px; font-weight:700; line-height:1.1; }}\
         .kpi-lbl {{ display:block; font-size:10px; font-weight:600; letter-spacing:.04em; text-transform:uppercase; color:var(--muted); margin-top:4px; }}\
         .kpi.completed .kpi-val {{ color:var(--green); }}\
         .kpi.running .kpi-val {{ color:var(--blue); }}\
         .kpi.failed .kpi-val {{ color:var(--red); }}\
         .lead {{ margin:0; padding:12px 14px; border-radius:8px; border:1px solid var(--border); background:var(--inset); \
           font-size:14px; line-height:1.6; white-space:pre-wrap; }}\
         .grid {{ display:grid; grid-template-columns:1fr 1fr; gap:12px; }}\
         .section {{ padding:12px 14px; border:1px solid var(--border); border-radius:8px; background:var(--inset); }}\
         .section h3 {{ margin:0 0 8px; font-size:11px; font-weight:700; letter-spacing:.05em; text-transform:uppercase; color:var(--muted); }}\
         .section p {{ margin:0; font-size:13px; line-height:1.6; white-space:pre-wrap; }}\
         .agents {{ display:grid; grid-template-columns:repeat(auto-fill,minmax(220px,1fr)); gap:10px; }}\
         .agent {{ display:flex; gap:10px; padding:10px 12px; border:1px solid var(--border); border-radius:8px; background:var(--surface); }}\
         .avatar {{ width:32px; height:32px; border-radius:999px; border:1px solid var(--border); background:var(--inset); \
           display:flex; align-items:center; justify-content:center; font-weight:700; font-size:13px; flex-shrink:0; }}\
         .agent-name {{ font-size:13px; font-weight:650; }}\
         .agent-meta {{ font-size:11px; color:var(--muted); }}\
         .agent-summary {{ margin:6px 0 0; font-size:12px; color:var(--muted); line-height:1.5; }}\
         table.runs {{ width:100%; border-collapse:collapse; font-size:12px; }}\
         table.runs th {{ text-align:left; font-size:10px; font-weight:700; letter-spacing:.04em; text-transform:uppercase; \
           color:var(--muted); padding:0 10px 8px 0; border-bottom:1px solid var(--border); }}\
         table.runs td {{ padding:8px 10px 8px 0; vertical-align:top; border-bottom:1px solid #f1f5f9; line-height:1.45; }}\
         table.runs tr:last-child td {{ border-bottom:none; }}\
         .status {{ display:inline-block; font-size:10px; font-weight:600; padding:2px 8px; border-radius:4px; border:1px solid var(--border); }}\
         .status.completed {{ color:var(--green); background:#f0fdf4; border-color:#bbf7d0; }}\
         .status.running {{ color:var(--blue); background:#eff6ff; border-color:#bae6fd; }}\
         .status.failed {{ color:var(--red); background:#fef2f2; border-color:#fecaca; }}\
         .status.muted {{ color:var(--muted); background:var(--inset); }}\
         .footnotes {{ display:grid; grid-template-columns:1fr 1fr; gap:12px; }}\
         ul.callouts {{ margin:0; padding:0; list-style:none; }}\
         ul.callouts li {{ position:relative; padding-left:14px; margin:0 0 8px; font-size:13px; line-height:1.55; }}\
         ul.callouts li::before {{ content:''; position:absolute; left:0; top:.55em; width:6px; height:6px; border-radius:999px; }}\
         .issues ul.callouts li::before {{ background:var(--red); }}\
         .tips ul.callouts li::before {{ background:var(--green); }}\
         @media print {{ body {{ background:#fff; padding:12mm; }} .report {{ box-shadow:none; }} }}\
         @media (max-width:720px) {{ .kpis {{ grid-template-columns:1fr 1fr; }} .grid, .footnotes {{ grid-template-columns:1fr; }} }}\
         </style></head><body>\
         <h1>{title}</h1>{body}</body></html>",
        title = html_escape(title),
    )
}

fn render_daily_report_html(rep: &DailyReport) -> String {
    let badge = if rep.llm_enhanced {
        r#"<span class="badge llm">LLM enhanced</span>"#
    } else {
        r#"<span class="badge">Rule baseline</span>"#
    };
    let generated = rep
        .generated_at
        .format("%Y-%m-%d %H:%M UTC")
        .to_string();

    let mut body = String::new();
    body.push_str(&format!(
        r#"<article class="report"><header class="report-header"><div><p class="eyebrow">AgentMirror · Daily</p>
        <h2 class="report-title">{date}</h2><p class="report-sub">{name}</p></div>
        <div class="meta-col">{badge}<div>Generated {generated}</div></div></header><div class="body">"#,
        date = html_escape(&rep.date),
        name = html_escape(&rep.display_name),
    ));

    body.push_str(&format!(
        r#"<div class="kpis"><div class="kpi completed"><span class="kpi-val">{c}</span><span class="kpi-lbl">Completed</span></div>
        <div class="kpi running"><span class="kpi-val">{r}</span><span class="kpi-lbl">In progress</span></div>
        <div class="kpi failed"><span class="kpi-val">{f}</span><span class="kpi-lbl">Failed</span></div>
        <div class="kpi turns"><span class="kpi-val">{t}</span><span class="kpi-lbl">LLM turns</span></div></div>"#,
        c = rep.runs_completed,
        r = rep.runs_running,
        f = rep.runs_failed,
        t = rep.total_turns,
    ));

    if !rep.summary.is_empty() {
        body.push_str(&format!(r#"<p class="lead">{summary}</p>"#, summary = html_escape(&rep.summary)));
    }

    let has_tasks = rep.tasks_overview.as_ref().is_some_and(|s| !s.is_empty());
    let has_progress = rep.progress_narrative.as_ref().is_some_and(|s| !s.is_empty());
    if has_tasks || has_progress {
        body.push_str(r#"<div class="grid">"#);
        if let Some(tasks) = &rep.tasks_overview {
            if !tasks.is_empty() {
                body.push_str(&format!(
                    r#"<section class="section"><h3>Tasks</h3><p>{tasks}</p></section>"#,
                    tasks = html_escape(tasks),
                ));
            }
        }
        if let Some(progress) = &rep.progress_narrative {
            if !progress.is_empty() {
                body.push_str(&format!(
                    r#"<section class="section"><h3>Progress</h3><p>{progress}</p></section>"#,
                    progress = html_escape(progress),
                ));
            }
        }
        body.push_str("</div>");
    }

    if !rep.agent_sections.is_empty() {
        body.push_str(r#"<section class="section"><h3>Agents</h3><div class="agents">"#);
        for a in &rep.agent_sections {
            let initial = a
                .display_name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            body.push_str(&format!(
                r#"<article class="agent"><div class="avatar">{initial}</div><div>
                <div class="agent-name">{name}</div><div class="agent-meta">{runs} runs</div>
                {summary}</div></article>"#,
                name = html_escape(&a.display_name),
                runs = a.run_count,
                summary = if a.summary.is_empty() {
                    String::new()
                } else {
                    format!(r#"<p class="agent-summary">{s}</p>"#, s = html_escape(&a.summary))
                },
            ));
        }
        body.push_str("</div></section>");
    }

    if !rep.run_summaries.is_empty() {
        body.push_str(r#"<section class="section"><h3>Task runs</h3><table class="runs"><thead><tr>
            <th>Goal</th><th>Status</th><th>Turns</th></tr></thead><tbody>"#);
        for r in &rep.run_summaries {
            let st = run_status_class(&r.status);
            body.push_str(&format!(
                r#"<tr><td>{goal}</td><td><span class="status {st}">{status}</span></td><td>{turns}</td></tr>"#,
                goal = html_escape(&r.goal),
                st = st,
                status = html_escape(&r.status),
                turns = r.turn_count,
            ));
        }
        body.push_str("</tbody></table></section>");
    }

    let has_issues = !rep.top_issues.is_empty();
    let has_tips = !rep.top_suggestions.is_empty();
    if has_issues || has_tips {
        body.push_str(r#"<div class="footnotes">"#);
        if has_issues {
            body.push_str(r#"<section class="section issues"><h3>Issues &amp; risks</h3><ul class="callouts">"#);
            for i in &rep.top_issues {
                body.push_str(&format!("<li>{}</li>", html_escape(i)));
            }
            body.push_str("</ul></section>");
        }
        if has_tips {
            body.push_str(r#"<section class="section tips"><h3>Recommendations</h3><ul class="callouts">"#);
            for s in &rep.top_suggestions {
                body.push_str(&format!("<li>{}</li>", html_escape(s)));
            }
            body.push_str("</ul></section>");
        }
        body.push_str("</div>");
    }

    body.push_str("</div></article>");
    body
}

fn run_status_class(status: &str) -> &'static str {
    let s = status.to_ascii_lowercase();
    if s.contains("complete") || s.contains("done") || s.contains("success") {
        "completed"
    } else if s.contains("fail") || s.contains("error") {
        "failed"
    } else if s.contains("run") {
        "running"
    } else {
        "muted"
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
