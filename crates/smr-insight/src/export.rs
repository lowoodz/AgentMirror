use crate::models::DailyReport;

pub fn daily_reports_html(reports: &[DailyReport], title: &str) -> String {
    let mut body = String::new();
    for rep in reports {
        body.push_str(&format!(
            "<section class=\"report\"><h2>{} — {}</h2>\
             <p class=\"meta\">{} done · {} running · {} failed · {} turns</p>\
             <p>{}</p>",
            html_escape(&rep.display_name),
            html_escape(&rep.date),
            rep.runs_completed,
            rep.runs_running,
            rep.runs_failed,
            rep.total_turns,
            html_escape(&rep.summary),
        ));
        if let Some(tasks) = &rep.tasks_overview {
            if !tasks.is_empty() {
                body.push_str(&format!("<h3>Tasks</h3><p>{}</p>", html_escape(tasks)));
            }
        }
        if let Some(progress) = &rep.progress_narrative {
            if !progress.is_empty() {
                body.push_str(&format!("<h3>Progress</h3><p>{}</p>", html_escape(progress)));
            }
        }
        if !rep.agent_sections.is_empty() {
            body.push_str("<h3>Agents</h3>");
            for a in &rep.agent_sections {
                body.push_str(&format!(
                    "<h4>{} ({} runs)</h4><p>{}</p>",
                    html_escape(&a.display_name),
                    a.run_count,
                    html_escape(&a.summary),
                ));
            }
        }
        if !rep.run_summaries.is_empty() {
            body.push_str("<h3>Runs</h3><ul>");
            for r in &rep.run_summaries {
                body.push_str(&format!(
                    "<li>{} · {} · {} turns</li>",
                    html_escape(&r.goal),
                    html_escape(&r.status),
                    r.turn_count,
                ));
            }
            body.push_str("</ul>");
        }
        if !rep.top_issues.is_empty() {
            body.push_str("<h3>Issues</h3><ul>");
            for i in &rep.top_issues {
                body.push_str(&format!("<li>{}</li>", html_escape(i)));
            }
            body.push_str("</ul>");
        }
        if !rep.top_suggestions.is_empty() {
            body.push_str("<h3>Suggestions</h3><ul>");
            for s in &rep.top_suggestions {
                body.push_str(&format!("<li>{}</li>", html_escape(s)));
            }
            body.push_str("</ul>");
        }
        body.push_str("</section>");
    }

    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"/>\
         <title>{title}</title>\
         <style>\
         body {{ font-family: system-ui, sans-serif; margin: 24px; color: #111; }}\
         h1 {{ font-size: 20px; margin-bottom: 16px; }}\
         h2 {{ font-size: 16px; margin: 0 0 8px; }}\
         h3 {{ font-size: 13px; margin: 12px 0 6px; color: #444; }}\
         .meta {{ color: #666; font-size: 12px; margin: 0 0 8px; }}\
         section.report {{ border: 1px solid #ddd; border-radius: 8px; padding: 16px; margin-bottom: 16px; page-break-inside: avoid; }}\
         ul {{ margin: 4px 0; padding-left: 20px; font-size: 13px; }}\
         p {{ font-size: 13px; line-height: 1.5; }}\
         @media print {{ body {{ margin: 12mm; }} }}\
         </style></head><body>\
         <h1>{title}</h1>{body}</body></html>",
        title = html_escape(title),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
