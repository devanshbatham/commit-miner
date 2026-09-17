use crate::model::*;
use clap::Args;
use serde::{Deserialize, Serialize};
/// Filters apply only to Jev scores; source and commit-message text are never matched.
#[derive(Args, Clone, Debug, Default)]
pub struct Filters {
    #[arg(long, value_delimiter=',', value_parser=selector, help="Show any selected family/category: security, bug, change, context, unclassified, cwe_79…")]
    pub only: Vec<String>,
    #[arg(long, value_delimiter=',', value_parser=cwe, help="Restrict to CWE IDs, e.g. 79,89 or CWE-862; combined with --only")]
    pub cwe: Vec<u32>,
    #[arg(long, value_parser=probability, help="Minimum Jev score, 0–1; defaults to the scan threshold")]
    pub min_probability: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub only: Vec<String>,
    pub cwe: Vec<u32>,
    pub min_probability: f64,
    pub matched: usize,
    pub classified: usize,
}
pub fn selector(s: &str) -> Result<String, String> {
    let id = s.trim().to_ascii_lowercase().replace('-', "_");
    let id = match id.as_str() {
        "security_fix" => "security",
        "bug_fix" => "bug",
        "insufficient_context" => "context",
        _ => &id,
    };
    if ["security", "bug", "change", "context", "unclassified"].contains(&id)
        || taxonomy().categories.iter().any(|c| c.id == id)
    {
        Ok(id.into())
    } else {
        Err(format!(
            "Unknown classification '{s}'. Run `commit-miner categories` for IDs."
        ))
    }
}
pub fn cwe(s: &str) -> Result<u32, String> {
    let id = s.trim().to_ascii_lowercase();
    let id = id
        .strip_prefix("cwe-")
        .or_else(|| id.strip_prefix("cwe_"))
        .unwrap_or(&id);
    let number = id
        .parse::<u32>()
        .map_err(|_| "Use a CWE number such as 79 or CWE-79".to_string())?;
    if taxonomy().categories.iter().any(|c| c.cwe == Some(number)) {
        Ok(number)
    } else {
        Err(format!(
            "CWE-{number} is not in the current Jev taxonomy. Run `commit-miner categories`."
        ))
    }
}
pub fn probability(s: &str) -> Result<f64, String> {
    s.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
        .ok_or_else(|| "Probability must be between 0 and 1".into())
}
impl Filters {
    pub fn active(&self) -> bool {
        !self.only.is_empty() || !self.cwe.is_empty() || self.min_probability.is_some()
    }
    pub fn cutoff(&self, threshold: f64) -> f64 {
        self.min_probability.unwrap_or(threshold)
    }
    pub fn matches(&self, r: &ResultRecord, threshold: f64) -> bool {
        if !r.reviewed() {
            return !self.active();
        }
        let cutoff = self.cutoff(threshold);
        let has = |id: &str| r.probability(id).is_some_and(|p| p >= cutoff);
        let family = |f: &str| {
            taxonomy()
                .categories
                .iter()
                .any(|c| c.family == f && has(&c.id))
        };
        let classified = || {
            has("security_fix")
                || has("bug_fix")
                || taxonomy().categories.iter().any(|c| has(&c.id))
        };
        let selected = self.only.is_empty()
            || self.only.iter().any(|id| match id.as_str() {
                "security" => has("security_fix") || family("security"),
                "bug" => has("bug_fix") || family("bug"),
                "change" => family("change"),
                "context" => has("insufficient_context"),
                "unclassified" => !classified(),
                _ => has(id),
            });
        let cwes = self.cwe.is_empty()
            || taxonomy()
                .categories
                .iter()
                .any(|c| c.cwe.is_some_and(|id| self.cwe.contains(&id)) && has(&c.id));
        let minimum = self.min_probability.is_none()
            || !self.only.is_empty()
            || !self.cwe.is_empty()
            || classified()
            || has("insufficient_context");
        selected && cwes && minimum
    }
    pub fn apply(&self, mut scan: Scan) -> Scan {
        if self.active() {
            let threshold = scan.summary.options.threshold;
            let classified = scan.results.len();
            scan.results.retain(|r| self.matches(r, threshold));
            let cutoff = self.cutoff(threshold);
            for r in &mut scan.results {
                let mut labels = taxonomy()
                    .categories
                    .iter()
                    .filter_map(|c| {
                        r.probability(&c.id)
                            .filter(|p| *p >= cutoff)
                            .map(|probability| Label {
                                id: c.id.clone(),
                                probability,
                            })
                    })
                    .collect::<Vec<_>>();
                labels.sort_by(|a, b| b.probability.total_cmp(&a.probability));
                r.categories = labels;
            }
            scan.selection = Some(Selection {
                only: self.only.clone(),
                cwe: self.cwe.clone(),
                min_probability: self.cutoff(threshold),
                matched: scan.results.len(),
                classified,
            });
        }
        scan
    }
    pub fn description(&self, threshold: f64) -> String {
        let mut parts = vec![];
        if !self.only.is_empty() {
            parts.push(self.only.join(", "));
        }
        if !self.cwe.is_empty() {
            parts.push(
                self.cwe
                    .iter()
                    .map(|n| format!("CWE-{n}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if self.active() {
            parts.push(format!("≥ {:.0}%", self.cutoff(threshold) * 100.));
        }
        parts.join(" · ")
    }
}
