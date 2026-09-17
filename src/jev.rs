use crate::{model::*, store};
use anyhow::{Context, Result, bail, ensure};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{Notify, mpsc::UnboundedSender};
use tokio_util::sync::CancellationToken;
pub const MAX_WORKERS: usize = 8;
pub const REQUEST_BUDGET: usize = 28_000;
#[derive(Debug)]
pub enum Event {
    CallStarted,
    CallFinished,
    Cache,
    Usage {
        model: String,
        input_tokens: u64,
        output_tokens: u64,
    },
    SectionProgress {
        sha: String,
        done: usize,
        total: usize,
    },
    Retry {
        attempt: usize,
        status: u16,
        delay: Duration,
    },
    Commit(Box<ResultRecord>),
}
struct GateState {
    active: usize,
    limit: usize,
    successes: usize,
    cooldown: Instant,
    next_start: Instant,
    spacing: Duration,
    recovered: Instant,
    queues: [std::collections::VecDeque<u64>; 2],
    next_ticket: u64,
}
struct Gate {
    maximum: usize,
    state: Mutex<GateState>,
    notify: Notify,
}
struct Permit(Arc<Gate>);
struct Waiter {
    gate: Arc<Gate>,
    queue: usize,
    ticket: u64,
    queued: bool,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        if self.queued {
            self.gate.state.lock().unwrap().queues[self.queue].retain(|t| *t != self.ticket);
            self.gate.notify.notify_waiters();
        }
    }
}
struct CallActivity(UnboundedSender<Event>);
impl Drop for CallActivity {
    fn drop(&mut self) {
        let _ = self.0.send(Event::CallFinished);
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut s = self.0.state.lock().unwrap();
        s.active -= 1;
        drop(s);
        self.0.notify.notify_waiters();
    }
}
impl Gate {
    fn new(maximum: usize) -> Arc<Self> {
        Arc::new(Self {
            maximum: maximum.clamp(1, MAX_WORKERS),
            state: Mutex::new(GateState {
                active: 0,
                limit: maximum.clamp(1, MAX_WORKERS),
                successes: 0,
                cooldown: Instant::now(),
                next_start: Instant::now(),
                spacing: Duration::ZERO,
                recovered: Instant::now(),
                queues: Default::default(),
                next_ticket: 0,
            }),
            notify: Notify::new(),
        })
    }
    async fn acquire(self: &Arc<Self>, c: &CancellationToken, quick: bool) -> Result<Permit> {
        ensure!(!c.is_cancelled(), "Cancelled");
        let queue = usize::from(!quick);
        let ticket = {
            let mut s = self.state.lock().unwrap();
            let ticket = s.next_ticket;
            s.next_ticket += 1;
            s.queues[queue].push_back(ticket);
            ticket
        };
        let mut waiter = Waiter {
            gate: self.clone(),
            queue,
            ticket,
            queued: true,
        };
        loop {
            ensure!(!c.is_cancelled(), "Cancelled");
            let wake = self.notify.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            let delay = {
                let mut s = self.state.lock().unwrap();
                let delay = s
                    .cooldown
                    .max(s.next_start)
                    .saturating_duration_since(Instant::now());
                let turn =
                    s.queues[queue].front() == Some(&ticket) && (quick || s.queues[0].is_empty());
                if delay.is_zero() && s.active < s.limit && turn {
                    s.queues[queue].pop_front();
                    waiter.queued = false;
                    s.active += 1;
                    s.next_start = Instant::now() + s.spacing;
                    drop(s);
                    self.notify.notify_waiters();
                    return Ok(Permit(self.clone()));
                }
                delay
            };
            tokio::select! {_=c.cancelled()=>bail!("Cancelled"),_=wake=>{},_=tokio::time::sleep(delay),if !delay.is_zero()=>{}}
        }
    }
    fn pressure(&self, delay: Duration, throttled: bool) {
        let mut s = self.state.lock().unwrap();
        // Several in-flight requests may report the same overload wave.
        if Instant::now() >= s.cooldown {
            s.limit = (s.limit / 2).max(1);
            if throttled {
                s.spacing = (s.spacing * 2)
                    .max(Duration::from_millis(100))
                    .min(Duration::from_secs(10));
            }
        }
        s.successes = 0;
        s.recovered = Instant::now();
        s.cooldown = s.cooldown.max(Instant::now() + delay);
        drop(s);
        self.notify.notify_waiters();
    }
    fn success(&self) {
        let mut s = self.state.lock().unwrap();
        if Instant::now() >= s.cooldown {
            s.successes += 1;
            if s.successes >= (s.limit * 2).max(4)
                && s.recovered.elapsed() >= Duration::from_secs(5)
            {
                s.limit = (s.limit + 1).min(self.maximum);
                s.spacing = if s.spacing <= Duration::from_millis(5) {
                    Duration::ZERO
                } else {
                    s.spacing.mul_f64(0.8)
                };
                s.recovered = Instant::now();
                s.successes = 0;
            }
        }
        drop(s);
        self.notify.notify_waiters();
    }
}
#[derive(Clone)]
pub struct Jev {
    http: reqwest::Client,
    key: String,
    endpoint: String,
    pub model: String,
    cache_dir: PathBuf,
    cache: bool,
    gate: Arc<Gate>,
    events: UnboundedSender<Event>,
}
pub fn validate_request(body: &Value) -> Result<()> {
    ensure!(
        body["model"].as_str().is_some_and(|s| !s.is_empty()),
        "Missing Jev model"
    );
    ensure!(
        body["state"].is_string() || body["state"].is_object() || body["state"].is_array(),
        "Jev state must be text, an object or an array"
    );
    let questions = body["questions"]
        .as_object()
        .context("Missing Jev questions")?;
    ensure!(!questions.is_empty(), "Jev request must contain questions");
    for (id, q) in questions {
        ensure!(
            q["type"] == "noul"
                && (q["instructions"].is_string()
                    || q["instructions"].is_object()
                    || q["instructions"].is_array()),
            "Invalid Noul question {id}"
        );
    }
    Ok(())
}
pub fn validate(body: &Value, response: &Value) -> Result<()> {
    validate_request(body)?;
    if let Some(usage) = response.get("usage") {
        ensure!(
            usage["input_tokens"].as_u64().is_some() && usage["output_tokens"].as_u64().is_some(),
            "Jev returned invalid token usage"
        );
    }
    ensure!(
        response["model"].is_string(),
        "Jev returned an invalid response model"
    );
    for id in body["questions"]
        .as_object()
        .context("Missing questions")?
        .keys()
    {
        let a = &response["answers"][id];
        ensure!(
            a["type"] == "noul"
                && a["noul"]
                    .as_f64()
                    .is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n)),
            "Jev returned a missing or invalid Noul answer for {id}"
        );
    }
    Ok(())
}
impl Jev {
    pub fn new(
        key: String,
        model: String,
        root: PathBuf,
        cache: bool,
        workers: usize,
        events: UnboundedSender<Event>,
    ) -> Result<Self> {
        let cache_dir = root.join("commit-cache");
        store::private_dir(&cache_dir)?;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(90))
                .connect_timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            key,
            endpoint: "https://api.typesafe.ai/v1/systemone".into(),
            model,
            cache_dir,
            cache,
            gate: Gate::new(workers),
            events,
        })
    }
    #[cfg(test)]
    pub fn endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }
    pub async fn evaluate(&self, body: &Value, c: &CancellationToken) -> Result<(Value, bool)> {
        validate_request(body)?;
        if serde_json::to_vec(body)?.len() <= REQUEST_BUDGET {
            return self.evaluate_one(body, c).await;
        }
        // Questions are independent in Jev. Reuse identical state while packing
        // the questions into as many bounded requests as necessary.
        let mut batches = vec![];
        let mut batch = body.clone();
        batch["questions"] = json!({});
        for (id, question) in body["questions"].as_object().unwrap() {
            batch["questions"][id] = question.clone();
            if serde_json::to_vec(&batch)?.len() > REQUEST_BUDGET {
                batch["questions"].as_object_mut().unwrap().remove(id);
                ensure!(
                    !batch["questions"].as_object().unwrap().is_empty(),
                    "State must be divided into smaller evidence sections"
                );
                batches.push(batch);
                batch = body.clone();
                batch["questions"] = json!({});
                batch["questions"][id] = question.clone();
                ensure!(
                    serde_json::to_vec(&batch)?.len() <= REQUEST_BUDGET,
                    "State must be divided into smaller evidence sections"
                );
            }
        }
        if !batch["questions"].as_object().unwrap().is_empty() {
            batches.push(batch);
        }
        let mut answers = serde_json::Map::new();
        let mut model = None;
        let mut cached = true;
        let mut responses = stream::iter(batches)
            .map(|batch| async move { self.evaluate_one(&batch, c).await })
            .buffer_unordered(self.gate.maximum);
        while let Some(response) = responses.next().await {
            let (r, hit) = response?;
            cached &= hit;
            let received = r["model"].as_str().unwrap();
            if let Some(prior) = &model {
                ensure!(prior == received, "Jev model changed during a split review");
            } else {
                model = Some(received.to_string());
            }
            answers.extend(r["answers"].as_object().unwrap().clone());
        }
        let response = json!({"model":model,"answers":answers});
        validate(body, &response)?;
        Ok((response, cached))
    }
    async fn evaluate_one(&self, body: &Value, c: &CancellationToken) -> Result<(Value, bool)> {
        ensure!(!c.is_cancelled(), "Cancelled");
        validate_request(body)?;
        let bytes = serde_json::to_vec(body)?;
        ensure!(
            bytes.len() <= REQUEST_BUDGET,
            "Jev request exceeds the request budget"
        );
        let cache = self.cache_dir.join(format!(
            "{}.json",
            hash(serde_json::to_vec(
                &json!({"version":"rust-commit-evidence-v1","body":body})
            )?)
        ));
        if self.cache
            && let Ok(data) = tokio::fs::read(&cache).await
            && let Ok(entry) = serde_json::from_slice::<Value>(&data)
        {
            let age = chrono::Utc::now().timestamp_millis() - entry["saved"].as_i64().unwrap_or(0);
            if (0..86_400_000).contains(&age) && validate(body, &entry["response"]).is_ok() {
                let _ = self.events.send(Event::Cache);
                return Ok((entry["response"].clone(), true));
            }
        }
        let response = self.send(body, c).await?;
        if self.cache {
            store::atomic_json(
                &cache,
                &json!({"saved":chrono::Utc::now().timestamp_millis(),"response":response}),
            )?;
        }
        Ok((response, false))
    }
    async fn send(&self, body: &Value, c: &CancellationToken) -> Result<Value> {
        for attempt in 0..4 {
            let quick = body["state"]["reviews"][0]["coverage"]["stage"] != "section_review";
            let _permit = self.gate.acquire(c, quick).await?;
            let _ = self.events.send(Event::CallStarted);
            let activity = CallActivity(self.events.clone());
            let work = async {
                let mut response = self
                    .http
                    .post(&self.endpoint)
                    .bearer_auth(&self.key)
                    .json(body)
                    .send()
                    .await?;
                let status = response.status().as_u16();
                let after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
                    .or_else(|| {
                        response
                            .headers()
                            .get("retry-after-ms")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<f64>().ok())
                            .filter(|v| v.is_finite() && *v >= 0.)
                            .map(|ms| (ms / 1000.).to_string())
                    });
                let mut bytes = vec![];
                if response.status().is_success() {
                    while let Some(chunk) = response.chunk().await? {
                        ensure!(
                            bytes.len() + chunk.len() <= 2 * 1024 * 1024,
                            "Jev response exceeds size limit"
                        );
                        bytes.extend_from_slice(&chunk);
                    }
                }
                Ok::<_, anyhow::Error>((status, after, bytes))
            };
            let result = tokio::select! {biased;_=c.cancelled()=>bail!("Cancelled"),r=work=>r};
            drop(activity);
            let (status, after, bytes) = match result {
                Ok(r) => r,
                Err(e) => {
                    if e.downcast_ref::<reqwest::Error>().is_none() {
                        return Err(e);
                    }
                    if attempt == 3 {
                        bail!(
                            "Could not reach Jev, or the request timed out after 4 attempts. Completed results are retained."
                        );
                    }
                    self.retry(attempt, 0, None);
                    continue;
                }
            };
            if (200..300).contains(&status) {
                let response: Value =
                    serde_json::from_slice(&bytes).context("Jev returned invalid JSON")?;
                validate(body, &response)?;
                if let Some(usage) = response.get("usage") {
                    let _ = self.events.send(Event::Usage {
                        model: response["model"].as_str().unwrap().into(),
                        input_tokens: usage["input_tokens"].as_u64().unwrap(),
                        output_tokens: usage["output_tokens"].as_u64().unwrap(),
                    });
                }
                self.gate.success();
                return Ok(response);
            }
            if status == 422 {
                bail!(
                    "Jev rejected the request schema (HTTP 422). Completed results are retained."
                );
            }
            if status == 401 || status == 403 {
                bail!("TypeSafe rejected the API key. Check your key and account access.");
            }
            if [429, 500, 502, 503, 504, 529].contains(&status) && attempt < 3 {
                self.retry(attempt, status, after.as_deref());
                continue;
            }
            bail!(
                "Jev returned HTTP {status}. The scan is incomplete; completed results are retained."
            );
        }
        bail!("Jev retry limit reached")
    }
    fn retry(&self, attempt: usize, status: u16, after: Option<&str>) {
        let requested = after.and_then(|s| {
            s.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && *x >= 0.)
                .map(|s| Duration::from_secs_f64(s.min(300.)))
                .or_else(|| {
                    httpdate::parse_http_date(s)
                        .ok()
                        .map(|d| d.duration_since(SystemTime::now()).unwrap_or_default())
                })
        });
        let jitter = Duration::from_millis((uuid::Uuid::new_v4().as_u128() % 500) as u64);
        let delay = requested
            .unwrap_or(Duration::from_secs(1 << attempt))
            .clamp(Duration::from_secs(1), Duration::from_secs(300))
            + jitter;
        self.gate.pressure(delay, status == 429 || status == 529);
        let _ = self.events.send(Event::Retry {
            attempt: attempt + 1,
            status,
            delay,
        });
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    #[tokio::test]
    async fn single_call_reviews_go_ahead_of_queued_split_work() {
        let gate = Gate::new(1);
        let cancel = CancellationToken::new();
        let occupied = gate.acquire(&cancel, false).await.unwrap();
        let slow_gate = gate.clone();
        let slow_cancel = cancel.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let slow_tx = tx.clone();
        let slow = tokio::spawn(async move {
            let _permit = slow_gate.acquire(&slow_cancel, false).await.unwrap();
            slow_tx.send("split").unwrap();
        });
        while gate.state.lock().unwrap().queues[1].is_empty() {
            tokio::task::yield_now().await;
        }
        let fast_gate = gate.clone();
        let fast_cancel = cancel.clone();
        let fast = tokio::spawn(async move {
            let _permit = fast_gate.acquire(&fast_cancel, true).await.unwrap();
            tx.send("single").unwrap();
        });
        while gate.state.lock().unwrap().queues[0].is_empty() {
            tokio::task::yield_now().await;
        }
        drop(occupied);
        assert_eq!(rx.recv().await.unwrap(), "single");
        assert_eq!(rx.recv().await.unwrap(), "split");
        fast.await.unwrap();
        slow.await.unwrap();
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }
    #[tokio::test]
    async fn caps_workers_and_cancels_queued_calls() {
        let gate = Gate::new(128);
        let cancel = CancellationToken::new();
        let mut permits = vec![];
        for _ in 0..MAX_WORKERS {
            permits.push(gate.acquire(&cancel, true).await.unwrap());
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(25), gate.acquire(&cancel, true))
                .await
                .is_err()
        );
        cancel.cancel();
        assert!(gate.acquire(&cancel, true).await.is_err());
        drop(permits);
        assert!(gate.acquire(&cancel, true).await.is_err());
    }
    #[tokio::test]
    async fn overload_wave_reduces_once_and_cooldown_is_interruptible() {
        let gate = Gate::new(8);
        gate.pressure(Duration::from_secs(60), true);
        gate.pressure(Duration::from_secs(60), true);
        assert_eq!(gate.state.lock().unwrap().limit, 4);
        assert_eq!(
            gate.state.lock().unwrap().spacing,
            Duration::from_millis(100)
        );
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            stop.cancel();
        });
        assert!(
            tokio::time::timeout(Duration::from_secs(1), gate.acquire(&cancel, true))
                .await
                .unwrap()
                .is_err()
        );
    }
    #[tokio::test]
    async fn throttling_spaces_request_starts_even_when_slots_are_free() {
        let gate = Gate::new(8);
        gate.pressure(Duration::ZERO, true);
        let cancel = CancellationToken::new();
        let first = gate.acquire(&cancel, true).await.unwrap();
        let started = Instant::now();
        let second = gate.acquire(&cancel, true).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(90));
        drop((first, second));
    }
}
