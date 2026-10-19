use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use rand::Rng;

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::FaultInjectionConfig;

/// Compiled, hot-path-friendly fault injection state.
struct Compiled {
    delay: Option<CompiledDelay>,
    abort: Option<CompiledAbort>,
}

struct CompiledDelay {
    ratio: f64,
    kind: DelayKind,
}

/// Delay bounds in milliseconds, matching the configuration and log units.
enum DelayKind {
    Fixed(u64),
    Range { min_ms: u64, max_ms: u64 },
}

struct CompiledAbort {
    ratio: f64,
    status_code: u16,
    /// Prepared once per generation: the configured abort body clones as a
    /// refcount bump on every injected fault.
    body: Option<Bytes>,
    content_type: String,
    headers: Vec<(String, String)>,
}

pub struct FaultInjection {
    compiled: Compiled,
}

impl FaultInjection {
    pub fn new(config: &FaultInjectionConfig) -> Self {
        let delay = config.delay.as_ref().map(|d| {
            let kind = if let Some(fixed) = d.fixed_ms {
                DelayKind::Fixed(fixed)
            } else {
                let min = d.min_ms.expect("validated: min_ms set when fixed_ms not set");
                let max = d.max_ms.expect("validated: max_ms set when min_ms set");
                DelayKind::Range {
                    min_ms: min,
                    max_ms: max,
                }
            };
            CompiledDelay { ratio: d.ratio, kind }
        });
        let abort = config.abort.as_ref().map(|a| CompiledAbort {
            ratio: a.ratio,
            status_code: a.status_code,
            body: a.body.clone().map(Bytes::from),
            content_type: a.content_type.clone(),
            headers: a
                .headers
                .as_ref()
                .map(|hs| hs.iter().map(|h| (h.name.clone(), h.value.clone())).collect())
                .unwrap_or_default(),
        });
        FaultInjection {
            compiled: Compiled { delay, abort },
        }
    }
}

/// Pure helper: deterministic endpoints do not sample; intermediate ratios
/// take exactly one Bernoulli sample.
fn decide<R: Rng + ?Sized>(rng: &mut R, ratio: f64) -> bool {
    if ratio <= 0.0 {
        false
    } else if ratio >= 1.0 {
        true
    } else {
        rng.random_bool(ratio)
    }
}

fn sample_delay<R: Rng>(rng: &mut R, kind: &DelayKind) -> u64 {
    match kind {
        DelayKind::Fixed(d) => *d,
        DelayKind::Range { min_ms, max_ms } => {
            if min_ms == max_ms {
                *min_ms
            } else {
                rng.random_range(*min_ms..=*max_ms)
            }
        }
    }
}

#[async_trait]
impl RequestFilter for FaultInjection {
    fn name(&self) -> &str {
        "FaultInjection"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        use crate::plugins::runtime::log::la;

        // Scope the RNG to a block so ThreadRng (!Send) is dropped before any await.
        let (delay_ms, abort_hit) = {
            let mut rng = rand::rng();
            // Independent dice for delay and abort.
            let delay_ms = self.compiled.delay.as_ref().and_then(|d| {
                if decide(&mut rng, d.ratio) {
                    Some(sample_delay(&mut rng, &d.kind))
                } else {
                    None
                }
            });
            let abort_hit = self.compiled.abort.as_ref().filter(|a| decide(&mut rng, a.ratio));
            (delay_ms, abort_hit)
        };

        if delay_ms.is_none() && abort_hit.is_none() {
            return PluginRunningResult::GoodNext;
        }

        if let Some(ms) = delay_ms {
            plugin_log.push_fmt("fault delay={}ms", la![ms]);
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }

        if let Some(a) = abort_hit {
            plugin_log.push_fmt("fault abort={}", la![a.status_code]);
            let _ = session.set_response_header("content-type", &a.content_type);
            for (name, value) in &a.headers {
                let _ = session.append_response_header(name, value);
            }
            return PluginRunningResult::ErrResponse {
                status: a.status_code,
                body: a.body.clone(),
            };
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/fault_injection/plugin_test.rs"]
mod tests;
