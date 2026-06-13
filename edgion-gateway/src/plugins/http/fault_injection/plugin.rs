use std::time::Duration;

use async_trait::async_trait;
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

enum DelayKind {
    Fixed(Duration),
    Range { min: Duration, max: Duration },
}

struct CompiledAbort {
    ratio: f64,
    status_code: u16,
    body: Option<String>,
    content_type: String,
    headers: Vec<(String, String)>,
}

pub struct FaultInjection {
    name: String,
    compiled: Compiled,
}

impl FaultInjection {
    pub fn new(config: &FaultInjectionConfig) -> Self {
        let delay = config.delay.as_ref().map(|d| {
            let kind = if let Some(fixed) = d.fixed_ms {
                DelayKind::Fixed(Duration::from_millis(fixed))
            } else {
                let min = d.min_ms.expect("validated: min_ms set when fixed_ms not set");
                let max = d.max_ms.expect("validated: max_ms set when min_ms set");
                DelayKind::Range {
                    min: Duration::from_millis(min),
                    max: Duration::from_millis(max),
                }
            };
            CompiledDelay { ratio: d.ratio, kind }
        });
        let abort = config.abort.as_ref().map(|a| CompiledAbort {
            ratio: a.ratio,
            status_code: a.status_code,
            body: a.body.clone(),
            content_type: a.content_type.clone(),
            headers: a
                .headers
                .as_ref()
                .map(|hs| hs.iter().map(|h| (h.name.clone(), h.value.clone())).collect())
                .unwrap_or_default(),
        });
        FaultInjection {
            name: "FaultInjection".to_string(),
            compiled: Compiled { delay, abort },
        }
    }
}

/// Pure helper: returns true with probability `ratio`. Sampling the RNG once.
/// Exposed for unit-testing distribution shape with a seeded RNG.
pub(crate) fn decide<R: Rng>(rng: &mut R, ratio: f64) -> bool {
    rng.random::<f64>() < ratio
}

fn sample_delay<R: Rng>(rng: &mut R, kind: &DelayKind) -> Duration {
    match kind {
        DelayKind::Fixed(d) => *d,
        DelayKind::Range { min, max } => {
            let lo = min.as_millis() as u64;
            let hi = max.as_millis() as u64;
            let pick = if lo == hi { lo } else { rng.random_range(lo..=hi) };
            Duration::from_millis(pick)
        }
    }
}

#[async_trait]
impl RequestFilter for FaultInjection {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        use crate::plugins::runtime::log::la;

        // Scope the RNG to a block so ThreadRng (!Send) is dropped before any await.
        let (delay_dur, abort_hit) = {
            let mut rng = rand::rng();
            // Independent dice for delay and abort.
            let delay_dur = self.compiled.delay.as_ref().and_then(|d| {
                if decide(&mut rng, d.ratio) {
                    Some(sample_delay(&mut rng, &d.kind))
                } else {
                    None
                }
            });
            let abort_hit = self.compiled.abort.as_ref().filter(|a| decide(&mut rng, a.ratio));
            (delay_dur, abort_hit)
        };

        if delay_dur.is_none() && abort_hit.is_none() {
            return PluginRunningResult::GoodNext;
        }

        if let Some(dur) = delay_dur {
            let ms = dur.as_millis().to_string();
            plugin_log.push_fmt("fault delay={}ms", la![ms.as_str()]);
            tokio::time::sleep(dur).await;
        }

        if let Some(a) = abort_hit {
            let status = a.status_code.to_string();
            plugin_log.push_fmt("fault abort={}", la![status.as_str()]);
            let _ = session.set_response_header("content-type", &a.content_type);
            for (name, value) in &a.headers {
                let _ = session.set_response_header(name, value);
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
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn decide_zero_is_never_true() {
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..1000 {
            assert!(!decide(&mut rng, 0.0));
        }
    }

    #[test]
    fn decide_one_is_always_true() {
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..1000 {
            assert!(decide(&mut rng, 1.0));
        }
    }

    #[test]
    fn decide_distribution_matches_ratio() {
        let mut rng = StdRng::seed_from_u64(0xC0FFEE);
        let trials = 10_000;
        let hits = (0..trials).filter(|_| decide(&mut rng, 0.3)).count();
        let observed = hits as f64 / trials as f64;
        assert!(
            (0.28..=0.32).contains(&observed),
            "expected ~0.30, observed {observed} ({hits}/{trials})"
        );
    }

    #[test]
    fn sample_delay_range_inclusive_bounds() {
        let mut rng = StdRng::seed_from_u64(7);
        let kind = DelayKind::Range {
            min: Duration::from_millis(100),
            max: Duration::from_millis(200),
        };
        for _ in 0..1000 {
            let d = sample_delay(&mut rng, &kind);
            assert!(d >= Duration::from_millis(100));
            assert!(d <= Duration::from_millis(200));
        }
    }

    #[test]
    fn sample_delay_fixed_constant() {
        let mut rng = StdRng::seed_from_u64(7);
        let kind = DelayKind::Fixed(Duration::from_millis(300));
        for _ in 0..50 {
            assert_eq!(sample_delay(&mut rng, &kind), Duration::from_millis(300));
        }
    }
}
