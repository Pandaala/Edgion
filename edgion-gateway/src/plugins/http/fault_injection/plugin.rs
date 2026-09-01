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
        "FaultInjection"
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{AbortConfig, FaultHeaderEntry};
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    struct PanicRng;

    impl RngCore for PanicRng {
        fn next_u32(&mut self) -> u32 {
            panic!("deterministic ratio sampled the RNG")
        }

        fn next_u64(&mut self) -> u64 {
            panic!("deterministic ratio sampled the RNG")
        }

        fn fill_bytes(&mut self, _dst: &mut [u8]) {
            panic!("deterministic ratio sampled the RNG")
        }
    }

    struct CountingRng {
        calls: usize,
        value: u64,
    }

    impl CountingRng {
        fn returning(value: u64) -> Self {
            Self { calls: 0, value }
        }
    }

    impl RngCore for CountingRng {
        fn next_u32(&mut self) -> u32 {
            self.calls += 1;
            self.value as u32
        }

        fn next_u64(&mut self) -> u64 {
            self.calls += 1;
            self.value
        }

        fn fill_bytes(&mut self, dst: &mut [u8]) {
            self.calls += 1;
            dst.fill(self.value as u8);
        }
    }

    fn abort_config(headers: Option<Vec<FaultHeaderEntry>>) -> FaultInjectionConfig {
        FaultInjectionConfig {
            delay: None,
            abort: Some(AbortConfig {
                ratio: 1.0,
                status_code: 503,
                body: Some("injected fault".to_string()),
                content_type: "application/problem+json".to_string(),
                headers,
            }),
        }
    }

    #[test]
    fn fault_injection_name_is_one_static_value() {
        let first = FaultInjection::new(&abort_config(None));
        let second = FaultInjection::new(&abort_config(None));

        assert_eq!(first.name(), "FaultInjection");
        assert_eq!(second.name(), "FaultInjection");
        assert!(std::ptr::eq(first.name(), second.name()));
    }

    #[test]
    fn decide_ratio_endpoints_do_not_sample_rng() {
        let mut rng = PanicRng;

        assert!(!decide(&mut rng, -1.0));
        assert!(!decide(&mut rng, 0.0));
        assert!(decide(&mut rng, 1.0));
        assert!(decide(&mut rng, 2.0));
    }

    #[test]
    fn decide_intermediate_ratio_takes_exactly_one_sample() {
        let mut hit_rng = CountingRng::returning(0);
        assert!(decide(&mut hit_rng, 0.5));
        assert_eq!(hit_rng.calls, 1);

        let mut miss_rng = CountingRng::returning(u64::MAX);
        assert!(!decide(&mut miss_rng, 0.5));
        assert_eq!(miss_rng.calls, 1);
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

    #[tokio::test]
    async fn abort_sets_content_type_then_appends_duplicate_headers_in_order() {
        use mockall::Sequence;

        let plugin = FaultInjection::new(&abort_config(Some(vec![
            FaultHeaderEntry {
                name: "set-cookie".to_string(),
                value: "a=1; Path=/".to_string(),
            },
            FaultHeaderEntry {
                name: "x-fault-step".to_string(),
                value: "middle".to_string(),
            },
            FaultHeaderEntry {
                name: "set-cookie".to_string(),
                value: "b=2; Path=/".to_string(),
            },
        ])));
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_header()
            .withf(|name, value| name == "content-type" && value == "application/problem+json")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header()
            .withf(|name, value| name == "set-cookie" && value == "a=1; Path=/")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header()
            .withf(|name, value| name == "x-fault-step" && value == "middle")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header()
            .withf(|name, value| name == "set-cookie" && value == "b=2; Path=/")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new("FaultInjection");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 503,
                body: Some("injected fault".to_string()),
            }
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            serde_json::json!(["fault abort=503"])
        );
    }
}
