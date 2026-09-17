use crate::model::*;
use anyhow::{Result, bail};
pub fn format(s: &str) -> Result<String> {
    let s = s.to_ascii_lowercase();
    if ["csv", "html"].contains(&s.as_str()) {
        Ok(s)
    } else {
        bail!("Choose an output format: csv or html")
    }
}
fn cell(value: &str) -> String {
    let formula =
        value.trim_start().starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r']);
    format!(
        "\"{}{}\"",
        if formula { "'" } else { "" },
        value.replace('"', "\"\"")
    )
}
pub fn render(scan: &Scan, format: &str) -> Result<String> {
    match format {
        "csv" => {
            let threshold = scan
                .selection
                .as_ref()
                .map(|s| s.min_probability)
                .unwrap_or(scan.summary.options.threshold);
            let mut rows = vec![vec![
                "Commit".into(),
                "Message".into(),
                "Type".into(),
                "CWE".into(),
                "Date".into(),
            ]];
            for r in &scan.results {
                let cwes = r.cwes(threshold);
                let (kind, family) = r.primary_classification(threshold);
                let date = if r.commit.committed_at.is_empty() {
                    &r.commit.date
                } else {
                    &r.commit.committed_at
                };
                rows.push(vec![
                    r.commit.sha.clone(),
                    r.commit.message.lines().next().unwrap_or("").into(),
                    kind,
                    if family == "security" && cwes.is_empty() {
                        "Unresolved".into()
                    } else {
                        cwes.iter()
                            .map(|n| format!("CWE-{n}"))
                            .collect::<Vec<_>>()
                            .join("; ")
                    },
                    date.chars().take(10).collect(),
                ]);
            }
            Ok(rows
                .iter()
                .map(|r| r.iter().map(|s| cell(s)).collect::<Vec<_>>().join(","))
                .collect::<Vec<_>>()
                .join("\r\n")
                + "\r\n")
        }
        "html" => {
            let data = serde_json::to_string(scan)?
                .replace('<', "\\u003c")
                .replace('>', "\\u003e")
                .replace('&', "\\u0026")
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029");
            let css = include_str!("../assets/report.css");
            let js = include_str!("../assets/report.js").replace("</script", "<\\/script");
            Ok(format!(
                "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src data:; img-src data:; connect-src 'none'; base-uri 'none'; form-action 'none'\"><title>commit-miner</title><style>{css}</style></head><body><div id=\"root\"></div><script id=\"report-data\" type=\"application/json\">{data}</script><script>{js}</script></body></html>"
            ))
        }
        _ => bail!("Unknown output format"),
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn spreadsheet() {
        assert_eq!(super::cell("=1+1"), "\"'=1+1\"");
        assert_eq!(super::cell("a,\"b\""), "\"a,\"\"b\"\"\"");
    }
}
