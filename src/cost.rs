use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input_per_million_usd: f64,
    pub output_per_million_usd: f64,
    pub checked_on: String,
}

fn price(model: &str) -> Option<Price> {
    // Versioned IDs only: an alias may move to a differently priced model.
    // https://docs.typesafe.ai/models (1.13.0)
    // https://docs.typesafe.ai/cookbooks/parallel_questions (1.12)
    match model {
        "jev-1.13.0" | "jev-1.12" => Some(Price {
            input_per_million_usd: 0.042,
            output_per_million_usd: 0.,
            checked_on: "2026-09-17".into(),
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub price: Option<Price>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CostEstimate {
    pub models: BTreeMap<String, ModelCost>,
}
impl CostEstimate {
    pub fn record(&mut self, model: &str, input: u64, output: u64) {
        let usage = self
            .models
            .entry(model.into())
            .or_insert_with(|| ModelCost {
                input_tokens: 0,
                output_tokens: 0,
                price: price(model),
            });
        usage.input_tokens += input;
        usage.output_tokens += output;
    }
    pub fn usd(&self) -> Option<f64> {
        if self.models.is_empty() {
            return None;
        }
        self.models.values().try_fold(0., |sum, usage| {
            let p = usage.price.as_ref()?;
            Some(
                sum + (usage.input_tokens as f64 * p.input_per_million_usd
                    + usage.output_tokens as f64 * p.output_per_million_usd)
                    / 1_000_000.,
            )
        })
    }
    pub fn label(&self) -> String {
        match self.usd() {
            Some(usd) if usd > 0. && usd < 0.000001 => "est. <$0.000001".into(),
            Some(usd) => format!("est. ${usd:.6}"),
            None => "cost unavailable".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calculates_reported_tokens_and_preserves_the_price_snapshot() {
        let mut cost = CostEstimate::default();
        cost.record("jev-1.13.0", 100_000, 99_999);
        assert!((cost.usd().unwrap() - 0.0042).abs() < 1e-12);
        assert_eq!(cost.label(), "est. $0.004200");
        cost.record("jev-1.13.0", 900_000, 1);
        let saved = serde_json::to_vec(&cost).unwrap();
        let restored: CostEstimate = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            restored.models["jev-1.13.0"]
                .price
                .as_ref()
                .unwrap()
                .checked_on,
            "2026-09-17"
        );
        assert!((restored.usd().unwrap() - 0.042).abs() < 1e-12);
    }
    #[test]
    fn unknown_models_and_tiny_costs_are_not_shown_as_free() {
        let mut cost = CostEstimate::default();
        cost.record("jev-1.13.0", 1, 0);
        assert_eq!(cost.label(), "est. <$0.000001");
        cost.record("jev-future", 10, 0);
        assert_eq!(cost.label(), "cost unavailable");
        assert!(price("jev-latest").is_none());
    }
}
