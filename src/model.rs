use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub type Scores = BTreeMap<String, f64>;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    pub first_parent: bool,
    pub threshold: f64,
    pub concurrency: usize,
    pub cache: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub date: String,
    pub author: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub committed_at: String,
    pub files: Vec<String>,
    pub merge: bool,
    #[serde(default)]
    pub excluded_files: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: String,
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub sha: String,
    pub path: String,
    pub header: String,
    pub lines: Vec<DiffLine>,
    pub part: usize,
    pub parts: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub evidence: Evidence,
    pub probabilities: Scores,
    pub cached: bool,
    pub model: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub probability: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    pub commit: Commit,
    pub status: String,
    pub evaluated: usize,
    pub total: usize,
    pub probabilities: Scores,
    pub categories: Vec<Label>,
    pub evidence: Vec<Segment>,
    pub warnings: Vec<String>,
    pub aggregation: String,
    pub review_coverage: String,
}
impl ResultRecord {
    pub fn pending(sha: &str) -> Self {
        Self {
            commit: Commit {
                sha: sha.into(),
                parents: vec![],
                date: String::new(),
                author: String::new(),
                message: "Awaiting review".into(),
                committed_at: String::new(),
                files: vec![],
                merge: false,
                excluded_files: 0,
            },
            status: "pending".into(),
            evaluated: 0,
            total: 0,
            probabilities: Scores::new(),
            categories: vec![],
            evidence: vec![],
            warnings: vec![],
            aggregation: "commit_review".into(),
            review_coverage: "unavailable".into(),
        }
    }
    pub fn reviewed(&self) -> bool {
        self.status == "complete" || self.status == "partial"
    }
    pub fn cwes(&self, threshold: f64) -> Vec<u32> {
        taxonomy()
            .categories
            .iter()
            .filter_map(|c| {
                c.cwe
                    .filter(|_| self.probability(&c.id).is_some_and(|p| p >= threshold))
            })
            .collect()
    }

    /// Presentation priority only; every semantic score still comes from Jev.
    pub fn primary_classification(&self, threshold: f64) -> (String, &'static str) {
        if !self.reviewed() {
            return (
                (if self.status == "failed" {
                    "Failed"
                } else {
                    "Not analyzed"
                })
                .into(),
                "status",
            );
        }
        let has = |id: &str| self.probability(id).is_some_and(|p| p >= threshold);
        if !self.cwes(threshold).is_empty() {
            return ("Security fix".into(), "security");
        }
        if has("security_fix")
            || taxonomy()
                .categories
                .iter()
                .any(|c| c.family == "security" && has(&c.id))
        {
            return ("Security review".into(), "security");
        }
        if has("bug_fix")
            || taxonomy()
                .categories
                .iter()
                .any(|c| c.family == "bug" && has(&c.id))
        {
            return ("Bug fix".into(), "bug");
        }
        if let Some(category) = taxonomy()
            .categories
            .iter()
            .filter(|c| c.family == "change" && has(&c.id))
            .max_by(|a, b| {
                self.probability(&a.id)
                    .unwrap_or(0.)
                    .total_cmp(&self.probability(&b.id).unwrap_or(0.))
            })
        {
            let label = match category.id.as_str() {
                "change_performance" => "Performance",
                "change_feature" => "Feature",
                "change_refactor" => "Refactor",
                "change_hardening" => "Hardening",
                "change_dependency" => "Dependency",
                "change_compatibility" => "Compatibility",
                "change_build" => "Build",
                "change_resilience" => "Reliability",
                "change_cleanup" => "Cleanup",
                "change_api" => "API change",
                "change_ux" => "UX",
                _ => &category.label,
            };
            return (label.into(), "change");
        }
        (
            (if self.review_coverage == "metadata" {
                "Metadata review"
            } else {
                "Unclassified"
            })
            .into(),
            "context",
        )
    }

    pub fn probability(&self, id: &str) -> Option<f64> {
        self.probabilities.get(id).copied().or_else(|| {
            self.categories
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.probability)
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Progress {
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub percent: f64,
    pub commits: usize,
    pub classified: usize,
    pub calls: usize,
    pub cached: usize,
    pub skipped: usize,
    pub failed: usize,
    pub metadata_only: usize,
    pub excluded_changes: usize,
    pub excluded_commits: usize,
    pub bugs: usize,
    pub security: usize,
    pub requests_per_second: f64,
    pub elapsed_seconds: f64,
    pub active: usize,
    pub sections: usize,
    pub reviewed_sections: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: Option<crate::cost::CostEstimate>,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: "Preparing history".into(),
            done: 0,
            total: 0,
            percent: 0.,
            commits: 0,
            classified: 0,
            calls: 0,
            cached: 0,
            skipped: 0,
            failed: 0,
            metadata_only: 0,
            excluded_changes: 0,
            excluded_commits: 0,
            bugs: 0,
            security: 0,
            requests_per_second: 0.,
            elapsed_seconds: 0.,
            active: 0,
            sections: 0,
            reviewed_sections: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub status: String,
    pub options: Options,
    pub started: String,
    pub progress: Progress,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<crate::filter::Selection>,
    pub summary: Summary,
    pub results: Vec<ResultRecord>,
    pub events: Vec<Value>,
    pub owner_pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub label: String,
    pub family: String,
    pub description: String,
    pub cwe: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Taxonomy {
    pub categories: Vec<Category>,
    pub core_questions: BTreeMap<String, String>,
}
pub fn taxonomy() -> &'static Taxonomy {
    static T: std::sync::OnceLock<Taxonomy> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/taxonomy.json")).expect("embedded taxonomy")
    })
}
pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
