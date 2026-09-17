use crate::model::*;
use clap::ValueEnum;
use console::{Style, Term, measure_text_width};
use std::io::IsTerminal;
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}
#[derive(Clone, Copy)]
pub struct Terminal {
    pub color: bool,
    pub width: usize,
}
pub fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !('\u{202a}'..='\u{202e}').contains(c)
                && !('\u{2066}'..='\u{2069}').contains(c)
        })
        .collect()
}
impl Terminal {
    pub fn new(mode: ColorMode, stderr: bool, plain: bool) -> Self {
        let tty = if stderr {
            std::io::stderr().is_terminal()
        } else {
            std::io::stdout().is_terminal()
        };
        let color = !plain
            && match mode {
                ColorMode::Always => true,
                ColorMode::Never => false,
                ColorMode::Auto => {
                    tty && std::env::var_os("NO_COLOR").is_none()
                        && std::env::var("TERM").is_ok_and(|t| t != "dumb")
                }
            };
        let term = if stderr {
            Term::stderr()
        } else {
            Term::stdout()
        };
        Self {
            color,
            width: term
                .size_checked()
                .map(|(_, w)| usize::from(w))
                .unwrap_or(100)
                .clamp(16, 120),
        }
    }
    pub fn ink(&self, s: &str, color: u8, bold: bool) -> String {
        let mut style = if color == 252 {
            Style::new()
        } else {
            Style::new().color256(color)
        }
        .force_styling(self.color);
        if bold {
            style = style.bold();
        }
        style.apply_to(clean(s)).to_string()
    }
    fn rule(&self, s: &str) -> String {
        self.ink(s, 244, false)
    }
    fn rows(&self, text: &str) -> Vec<String> {
        wrap(&clean(text), self.width.saturating_sub(6).max(1))
    }
    fn body(&self, out: &mut Vec<String>, text: &str, color: u8, bold: bool) {
        for row in self.rows(text) {
            out.push(format!(
                "  {} {}",
                self.rule("│"),
                self.ink(&row, color, bold)
            ));
        }
    }
    pub fn activity(&self, message: &str) -> String {
        self.rows(message)
            .iter()
            .enumerate()
            .map(|(i, row)| {
                format!(
                    "  {} {}",
                    self.ink(if i == 0 { "›" } else { " " }, 141, true),
                    self.ink(row, 244, false)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn heading(&self, title: &str, subtitle: &str) -> String {
        let mut out = vec![format!("\n  {}", self.ink(title, 141, true))];
        for row in self.rows(subtitle) {
            out.push(format!("  {}", self.ink(&row, 244, false)));
        }
        out.push(String::new());
        out.join("\n")
    }
    pub fn record(&self, r: &ResultRecord, threshold: f64) -> String {
        let mut labels = taxonomy()
            .categories
            .iter()
            .filter_map(|c| {
                r.probability(&c.id)
                    .filter(|p| *p >= threshold)
                    .map(|p| (c, p))
            })
            .collect::<Vec<_>>();
        labels.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut out = vec![format!(
            "  {} {}",
            self.rule("╭─"),
            self.ink(
                &r.commit.sha.chars().take(10).collect::<String>(),
                141,
                true
            )
        )];
        self.body(
            &mut out,
            r.commit.message.lines().next().unwrap_or("(no message)"),
            252,
            true,
        );
        if !r.reviewed() {
            self.body(
                &mut out,
                if r.status == "failed" {
                    "FAILED"
                } else {
                    "NOT ANALYZED"
                },
                179,
                true,
            );
            for warning in &r.warnings {
                self.body(&mut out, warning, 244, false);
            }
            out.push(format!("  {}\n", self.rule("╰─")));
            return out.join("\n");
        }
        let cwes = r
            .cwes(threshold)
            .iter()
            .map(|id| format!("CWE-{id}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut core = false;
        for (id, label, color) in [
            ("security_fix", "SECURITY FIX", 204),
            ("bug_fix", "BUG FIX", 179),
        ] {
            if let Some(p) = r.probability(id).filter(|p| *p >= threshold) {
                core = true;
                let label = if id == "security_fix" {
                    if cwes.is_empty() {
                        "SECURITY REVIEW · CWE unresolved".into()
                    } else {
                        format!("SECURITY FIX · {cwes}")
                    }
                } else {
                    label.into()
                };
                self.body(&mut out, &format!("{label}  {:.0}%", p * 100.), color, true);
            }
        }
        if labels.is_empty() && !core {
            self.body(&mut out, "Unclassified", 244, false);
        }
        for (c, p) in labels {
            let cwe = c.cwe.map(|n| format!("CWE-{n} · ")).unwrap_or_default();
            self.body(
                &mut out,
                &format!(
                    "{}  {cwe}{}  {:.0}%",
                    c.family.to_uppercase(),
                    c.label,
                    p * 100.
                ),
                family_color(&c.family),
                false,
            );
        }
        if let Some(score) = r
            .probabilities
            .get("insufficient_context")
            .filter(|p| **p >= threshold)
        {
            self.body(
                &mut out,
                &format!("NEEDS CONTEXT  {:.0}%", score * 100.),
                141,
                false,
            );
        }
        let date = if r.commit.committed_at.is_empty() {
            &r.commit.date
        } else {
            &r.commit.committed_at
        };
        self.body(
            &mut out,
            &format!(
                "{} · {} · {} files",
                date.chars().take(10).collect::<String>(),
                r.commit.author,
                r.commit.files.len()
            ),
            244,
            false,
        );
        if r.status == "partial" {
            self.body(
                &mut out,
                &format!(
                    "{} · {} / {} sections",
                    "Partial evidence", r.evaluated, r.total
                ),
                179,
                false,
            );
        }
        out.push(format!("  {}\n", self.rule("╰─")));
        out.join("\n")
    }
    pub fn diff(&self, r: &ResultRecord) -> String {
        let mut out = vec![];
        for segment in &r.evidence {
            let e = &segment.evidence;
            out.push(self.heading(&e.path, &e.header));
            for l in &e.lines {
                let color = match l.kind.as_str() {
                    "added" => 78,
                    "removed" => 204,
                    _ => 244,
                };
                let sign = match l.kind.as_str() {
                    "added" => "+",
                    "removed" => "−",
                    _ => " ",
                };
                let coords = format!(
                    "{:>5} {:>5} {sign} ",
                    l.old_line.map(|n| n.to_string()).unwrap_or_default(),
                    l.new_line.map(|n| n.to_string()).unwrap_or_default()
                );
                let width = self
                    .width
                    .saturating_sub(measure_text_width(&coords) + 2)
                    .max(1);
                for (i, row) in wrap(&clean(&l.text), width).into_iter().enumerate() {
                    out.push(format!(
                        "  {}",
                        self.ink(
                            &format!(
                                "{}{}",
                                if i == 0 {
                                    coords.clone()
                                } else {
                                    " ".repeat(measure_text_width(&coords))
                                },
                                row
                            ),
                            color,
                            false
                        )
                    ));
                }
            }
        }
        out.join("\n")
    }
    pub fn summary(&self, scan: &Scan, shown: usize, filtered: bool) -> String {
        let p = &scan.summary.progress;
        let color = if scan.summary.status == "completed" {
            78
        } else {
            179
        };
        let mut out = vec![format!("\n  {}", self.ink(&p.phase, color, true))];
        let counts = format!(
            "{} classified · {} security · {} bugs · {} metadata-only · {} failed",
            p.classified, p.security, p.bugs, p.metadata_only, p.failed
        );
        for row in self.rows(&counts) {
            out.push(format!("  {}", self.ink(&row, 244, false)));
        }
        if p.calls > 0 || p.cached > 0 {
            let mut usage = format!("{} calls · {} cached", p.calls, p.cached);
            if p.input_tokens > 0 || p.output_tokens > 0 {
                usage.push_str(&format!(
                    " · {} in / {} out tokens",
                    p.input_tokens, p.output_tokens
                ));
            }
            if p.calls == 0 {
                usage.push_str(" · est. $0.000000");
            } else {
                usage.push_str(&format!(
                    " · {}",
                    p.cost
                        .as_ref()
                        .map(|c| c.label())
                        .unwrap_or_else(|| "cost unavailable".into())
                ));
            }
            for row in self.rows(&usage) {
                out.push(format!("  {}", self.ink(&row, 244, false)));
            }
        }
        if filtered {
            out.push(format!(
                "  {}",
                self.ink(
                    &format!("{shown} matching / {} classified", p.classified),
                    141,
                    false
                )
            ));
        }
        out.push(format!(
            "  {} {}\n",
            self.rule("scan"),
            self.ink(&scan.summary.id, 244, false)
        ));
        out.join("\n")
    }
    pub fn table(&self, records: &[&ResultRecord], threshold: f64) -> String {
        let width = self.width.saturating_sub(2).max(38);
        let hash = 7;
        let date = 10;
        let kind = if width >= 88 {
            26
        } else if width >= 62 {
            18
        } else {
            8
        };
        let message = width.saturating_sub(hash + date + kind + 6).max(7);
        let widths = [hash, message, kind, date];
        let headers = ["COMMIT", "MESSAGE", "TYPE / CWE", "DATE"];
        let row = |cells: [String; 4], colors: [u8; 4], bold: bool| {
            let wrapped = cells
                .iter()
                .zip(widths)
                .map(|(s, w)| wrap_words(&clean(s), w))
                .collect::<Vec<_>>();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            (0..height)
                .map(|n| {
                    let cells = (0..4)
                        .map(|i| {
                            let text = wrapped[i].get(n).map(String::as_str).unwrap_or("");
                            let pad = widths[i].saturating_sub(measure_text_width(text));
                            self.ink(&format!("{text}{}", " ".repeat(pad)), colors[i], bold)
                        })
                        .collect::<Vec<_>>()
                        .join("  ");
                    format!("  {}", cells.trim_end())
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut out = vec![
            self.heading("Commits", &format!("{} results", records.len())),
            row(headers.map(str::to_string), [244; 4], true),
            format!("  {}", self.rule(&"─".repeat(width))),
        ];
        for r in records {
            let cwes = r.cwes(threshold);
            let (mut kind, family) = r.primary_classification(threshold);
            let color = if family == "status" {
                179
            } else if family == "context" {
                244
            } else {
                family_color(family)
            };
            if family == "security" {
                kind.push_str(" · ");
                kind.push_str(&if cwes.is_empty() {
                    "CWE unresolved".into()
                } else {
                    cwes.iter()
                        .map(|n| format!("CWE-{n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                });
            }
            let stamp = if r.commit.committed_at.is_empty() {
                &r.commit.date
            } else {
                &r.commit.committed_at
            };
            out.push(row(
                [
                    r.commit.sha.chars().take(7).collect(),
                    r.commit.message.lines().next().unwrap_or("").into(),
                    kind,
                    if stamp.is_empty() {
                        "—".into()
                    } else {
                        stamp.chars().take(10).collect()
                    },
                ],
                [141, 252, color, 244],
                false,
            ));
        }
        out.push(String::new());
        out.join("\n")
    }
    pub fn progress(&self, p: &Progress) -> String {
        let seconds = p.elapsed_seconds.max(0.) as u64;
        let elapsed = format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        );
        let suffix = format!(" {:>3.0}% · elapsed {elapsed}", p.percent);
        let width = self
            .width
            .saturating_sub(2 + measure_text_width(&suffix))
            .max(1);
        let fraction = (p.percent / 100.).clamp(0., 1.);
        let filled = (fraction * width as f64).round() as usize;
        format!(
            "  {}{}{}\n",
            self.ink(
                &"━".repeat(filled),
                if p.percent >= 100. { 78 } else { 141 },
                false
            ),
            self.rule(&"─".repeat(width - filled)),
            self.ink(&suffix, 244, false)
        )
    }
    pub fn categories(&self) -> String {
        let mut out = vec![self.heading(
            "Classifications",
            "security · bug · change · context · unclassified",
        )];
        for family in ["security", "bug", "change"] {
            out.push(format!(
                "  {}",
                self.ink(&family.to_uppercase(), family_color(family), true)
            ));
            for c in taxonomy().categories.iter().filter(|c| c.family == family) {
                let cwe = c.cwe.map(|n| format!(" · CWE-{n}")).unwrap_or_default();
                if self.width >= 88 {
                    out.push(format!(
                        "  {}  {}",
                        self.ink(&format!("{:<24}", c.id), family_color(family), false),
                        self.ink(&format!("{}{cwe}", c.label), 252, false)
                    ));
                } else {
                    out.push(format!(
                        "  {}",
                        self.ink(&c.id, family_color(family), false)
                    ));
                    for row in self.rows(&format!("{}{cwe}", c.label)) {
                        out.push(format!("    {}", self.ink(&row, 244, false)));
                    }
                }
            }
            out.push(String::new());
        }
        out.join("\n")
    }
    pub fn saved(&self, scan: &Scan) -> String {
        let mut out = vec![format!("  {}", self.ink(&scan.summary.id, 141, true))];
        for row in self.rows(&format!(
            "{} · {} · {} classified",
            scan.summary.started.chars().take(10).collect::<String>(),
            scan.summary.status,
            scan.summary.progress.classified
        )) {
            out.push(format!("    {}", self.ink(&row, 244, false)));
        }
        for row in self.rows(&scan.summary.options.source) {
            out.push(format!("    {}", self.ink(&row, 252, false)));
        }
        out.push(String::new());
        out.join("\n")
    }
}
fn family_color(s: &str) -> u8 {
    match s {
        "security" => 204,
        "bug" => 179,
        "change" => 80,
        _ => 244,
    }
}
// Wrap by display columns, so wide Unicode and long paths cannot push cards off screen.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![];
    let mut row = String::new();
    let mut size = 0;
    for c in text.chars() {
        let n = measure_text_width(&c.to_string());
        if size + n > width && !row.is_empty() {
            rows.push(row);
            row = String::new();
            size = 0;
        }
        row.push(c);
        size += n;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![];
    let mut row = String::new();
    for word in text.split_whitespace() {
        for part in wrap(word, width) {
            if !row.is_empty() && measure_text_width(&row) + 1 + measure_text_width(&part) > width {
                rows.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(&part);
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}
