use super::*;
use edgion_resources::plugin_configs::ExtProcConfig;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;

pub(in super::super) fn actual_owner(config: &ExtProcConfig) -> edgion_resources::EdgionPlugins {
    serde_json::from_value(
        serde_json::json!({"metadata":{"name":"ext-proc-test","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"ExtProc","config":config}]}}),
    )
    .unwrap()
}

pub(in super::super) fn actual_client(config: &ExtProcConfig) -> Result<ExtProcClient, String> {
    let owner = actual_owner(config);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    ExtProcClient::new(
        &entry,
        &edgion_common::link_sys_core::outbound_tls::outbound_global_default(),
    )
}

use edgion_common::ext_proc::proto::common_response::ResponseStatus;
use edgion_common::ext_proc::proto::CommonResponse;
use edgion_common::ext_proc::proto::{
    processing_request, BodyResponse, HeadersResponse, HttpBody, HttpHeaders, ImmediateResponse,
};
use edgion_resources::plugin_configs::GrpcServiceConfig;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};

fn install_test_crypto_provider() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn tls_cfg() -> ExtProcConfig {
    ExtProcConfig {
        grpc_service: GrpcServiceConfig {
            target: "ep:9000".to_string(),
            timeout_ms: 200,
            tls: TlsConfig {
                enabled: true,
                ..Default::default()
            },
        },
        ..Default::default()
    }
}

fn unsupported_identity_cases() -> Vec<(OutboundTlsValidation, bool, &'static str)> {
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};

    let hostname = OutboundTlsValidation {
        hostname: Some("private-processor.example".to_string()),
        ..Default::default()
    };
    let san = OutboundTlsValidation {
        subject_alt_names: Some(vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("private-processor.example".to_string()),
            uri: None,
        }]),
        ..Default::default()
    };
    let hostname_error = "ext_proc TLS validation.hostname is unsupported; remove the hostname override";
    let san_error = "ext_proc TLS validation.subjectAltNames is unsupported; remove the SAN policy";
    vec![
        (hostname.clone(), true, hostname_error),
        (
            OutboundTlsValidation {
                hostname: Some(String::new()),
                ..Default::default()
            },
            true,
            hostname_error,
        ),
        (san.clone(), true, san_error),
        (
            OutboundTlsValidation {
                hostname: hostname.hostname,
                ..san.clone()
            },
            true,
            hostname_error,
        ),
        (san, false, san_error),
    ]
}

#[test]
fn unsupported_tls_identity_is_rejected_during_construction() {
    for (validation, verify, expected) in unsupported_identity_cases() {
        let mut config = tls_cfg();
        config.grpc_service.tls.outbound = OutboundTlsLocal {
            verify: Some(verify),
            validation: Some(validation),
            ..Default::default()
        };
        assert_eq!(
            actual_client(&config).err().expect("identity policy must fail"),
            expected
        );
    }
}

#[test]
fn effective_tls_identity_guard_respects_inheritance_and_whole_block_override() {
    use edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls_with_defaults;
    use edgion_resources::resources::common::OutboundTlsConfig;

    for (validation, verify, expected) in unsupported_identity_cases() {
        let defaults = OutboundTlsConfig {
            verify,
            validation: Some(validation),
            ..Default::default()
        };
        let inherited = resolve_outbound_tls_with_defaults(&OutboundTlsLocal::default(), None, None, true, &defaults)
            .expect("identity policy is preserved by the shared resolver");
        assert_eq!(validate_effective_tls_capabilities(&inherited).unwrap_err(), expected);
        if !verify {
            assert!(
                inherited.verify,
                "a nonempty SAN policy forces verification before rejection"
            );
        }

        // A local validation block replaces every field of the global block.
        for subject_alt_names in [None, Some(Vec::new())] {
            let local = OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    subject_alt_names,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let replaced = resolve_outbound_tls_with_defaults(&local, None, None, true, &defaults).unwrap();
            validate_effective_tls_capabilities(&replaced).unwrap();
            assert_eq!(replaced.verify, verify);
            assert_eq!(
                build_rustls_config(&replaced).unwrap().alpn_protocols,
                vec![b"h2".to_vec()]
            );
        }
    }
}

#[test]
fn supported_effective_tls_policies_build_without_global_state_changes() {
    use edgion_common::link_sys_core::outbound_tls::resolve_outbound_tls_with_defaults;
    use edgion_resources::constants::secret_keys::tls as tls_keys;
    use edgion_resources::resources::common::OutboundTlsConfig;
    use k8s_openapi::api::core::v1::Secret;

    let defaults = OutboundTlsConfig::default();
    for verify in [true, false] {
        for validation in [
            None,
            Some(OutboundTlsValidation::default()),
            Some(OutboundTlsValidation {
                subject_alt_names: Some(Vec::new()),
                ..Default::default()
            }),
        ] {
            let local = OutboundTlsLocal {
                verify: Some(verify),
                validation,
                ..Default::default()
            };
            let resolved = resolve_outbound_tls_with_defaults(&local, None, None, true, &defaults).unwrap();
            validate_effective_tls_capabilities(&resolved).unwrap();
            assert_eq!(resolved.verify, verify);
            assert!(matches!(resolved.ca, EffectiveCa::System));
            assert_eq!(
                build_rustls_config(&resolved).unwrap().alpn_protocols,
                vec![b"h2".to_vec()]
            );
        }
    }

    let identity = rcgen::generate_simple_self_signed(vec!["processor.example".to_string()]).unwrap();
    let certificate = identity.cert.pem().into_bytes();
    let secret = Secret {
        data: Some(std::collections::BTreeMap::from([
            (
                tls_keys::CA_CERT.to_string(),
                k8s_openapi::ByteString(certificate.clone()),
            ),
            (tls_keys::CERT.to_string(), k8s_openapi::ByteString(certificate.clone())),
            (
                tls_keys::KEY.to_string(),
                k8s_openapi::ByteString(identity.signing_key.serialize_pem().into_bytes()),
            ),
        ])),
        ..Default::default()
    };
    let mut local = OutboundTlsLocal {
        verify: Some(true),
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![CustomCACertificateRef {
                kind: "Secret".to_string(),
                name: "processor-ca".to_string(),
                group: Some(String::new()),
                namespace: None,
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let resolved = resolve_outbound_tls_with_defaults(
        &local,
        Some(std::slice::from_ref(&secret)),
        Some(&secret),
        true,
        &defaults,
    )
    .unwrap();
    validate_effective_tls_capabilities(&resolved).unwrap();
    assert!(matches!(&resolved.ca, EffectiveCa::Custom(bundles) if bundles == &vec![certificate]));
    assert!(resolved.client_identity.is_some());
    let config = build_rustls_config(&resolved).unwrap();
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec()]);
    assert!(config.client_auth_cert_resolver.has_certs());

    local.verify = Some(false);
    let unverified = resolve_outbound_tls_with_defaults(
        &local,
        Some(std::slice::from_ref(&secret)),
        Some(&secret),
        true,
        &defaults,
    )
    .unwrap();
    validate_effective_tls_capabilities(&unverified).unwrap();
    assert!(!unverified.verify);
    assert!(
        unverified.client_identity.is_none(),
        "canonical skip-verify omits mTLS identity"
    );
    let config = build_rustls_config(&unverified).unwrap();
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec()]);
    assert!(!config.client_auth_cert_resolver.has_certs());
}

#[tokio::test]
async fn unsupported_tls_identity_compilation_seals_generic_rejection_for_both_failure_modes() {
    use crate::plugins::http::compile_plugins;
    use crate::plugins::runtime::log::PluginLog;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::filters::PluginRunningResult;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry};
    use edgion_resources::resources::EdgionPlugins;

    for failure_mode_allow in [false, true] {
        for (validation, verify, _) in unsupported_identity_cases() {
            let mut config = tls_cfg();
            config.failure_mode_allow = failure_mode_allow;
            config.grpc_service.tls.outbound = OutboundTlsLocal {
                verify: Some(verify),
                validation: Some(validation),
                ..Default::default()
            };
            let resource = EdgionPlugins {
                metadata: kube::core::ObjectMeta {
                    name: Some("unsupported-identity".into()),
                    namespace: Some("default".into()),
                    ..Default::default()
                },
                spec: EdgionPluginsSpec {
                    current_status: None,
                    request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::ExtProc(config))]),
                    ..Default::default()
                },
                status: None,
            };
            let compiled = compile_plugins(&resource);
            assert!(!compiled.is_activatable);
            assert!(!compiled.retain_previous_on_invalid_update);
            assert_eq!(
                compiled.preparse_errors,
                vec!["requestPlugins[0] (ExtProc): plugin construction failed"]
            );
            let first = compiled
                .plugin_runtime
                .request_plugins_iter()
                .next()
                .expect("sealed reject");
            assert_eq!(first.name(), "RejectInvalidConfig");
            let result = first
                .run_request(
                    &mut MockPluginSession::new(),
                    &mut PluginLog::new("RejectInvalidConfig"),
                )
                .await;
            match result {
                PluginRunningResult::ErrResponse { status, body } => {
                    assert_eq!(status, 500);
                    assert_eq!(body.unwrap().as_ref(), br#"{"message":"plugin configuration error"}"#);
                }
                _ => panic!("invalid TLS identity policy must reject before request processing"),
            }
        }
    }
}

#[test]
fn processing_phase_accepts_only_matching_response() {
    let header_request = ProcessingRequest {
        request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
    };
    let body_request = ProcessingRequest {
        request: Some(processing_request::Request::RequestBody(HttpBody::default())),
    };
    let header_response = ProcessingResponse {
        response: Some(processing_response::Response::RequestHeaders(HeadersResponse::default())),
        mode_override: None,
    };
    let body_response = ProcessingResponse {
        response: Some(processing_response::Response::RequestBody(BodyResponse::default())),
        mode_override: None,
    };

    let header_phase = ProcessingPhase::from_request(&header_request).unwrap();
    let body_phase = ProcessingPhase::from_request(&body_request).unwrap();
    assert!(header_phase.accepts(&header_response));
    assert!(!header_phase.accepts(&body_response));
    assert!(body_phase.accepts(&body_response));
    assert!(!body_phase.accepts(&header_response));
}

#[test]
fn immediate_response_is_terminal_not_a_phase_verdict() {
    let request = ProcessingRequest {
        request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
    };
    let immediate = ProcessingResponse {
        response: Some(processing_response::Response::ImmediateResponse(
            ImmediateResponse::default(),
        )),
        mode_override: None,
    };

    assert!(!ProcessingPhase::from_request(&request).unwrap().accepts(&immediate));
    assert!(matches!(
        immediate.response,
        Some(processing_response::Response::ImmediateResponse(_))
    ));
}

#[test]
fn standalone_mode_override_is_not_a_phase_verdict() {
    let request = ProcessingRequest {
        request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
    };
    let standalone = ProcessingResponse {
        response: None,
        mode_override: Some(Default::default()),
    };

    assert!(!ProcessingPhase::from_request(&request).unwrap().accepts(&standalone));
}

#[tokio::test]
async fn equivalent_authorities_and_message_timeouts_share_target_runtime() {
    let mut first = ExtProcConfig::default();
    first.grpc_service.target = "EXAMPLE.com:9000".to_string();
    first.grpc_service.timeout_ms = 50;
    let mut second = first.clone();
    second.grpc_service.target = "example.COM:9000".to_string();
    second.grpc_service.timeout_ms = 500;
    let first = actual_client(&first).unwrap();
    let second = actual_client(&second).unwrap();
    assert!(Arc::ptr_eq(&first.runtime, &second.runtime));
}

#[tokio::test]
async fn distinct_effective_tls_policy_does_not_share_target_runtime() {
    install_test_crypto_provider();
    let mut verified = tls_cfg();
    verified.grpc_service.tls.outbound.verify = Some(true);
    let mut unverified = verified.clone();
    unverified.grpc_service.tls.outbound.verify = Some(false);
    let verified = actual_client(&verified).unwrap();
    let unverified = actual_client(&unverified).unwrap();
    assert!(!Arc::ptr_eq(&verified.runtime, &unverified.runtime));
}

#[tokio::test]
async fn inherited_tls_hot_change_changes_target_runtime_identity() {
    use edgion_common::link_sys_core::outbound_tls::{outbound_global_default, set_outbound_global_default};
    use edgion_resources::resources::common::OutboundTlsConfig;

    static GLOBAL_TLS_TEST: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _guard = GLOBAL_TLS_TEST.lock();
    install_test_crypto_provider();
    let original = outbound_global_default();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig::default(),
    ));
    let inherited_verified = actual_client(&tls_cfg()).unwrap();
    set_outbound_global_default(edgion_common::link_sys_core::outbound_tls::test_util::snapshot(
        OutboundTlsConfig {
            verify: false,
            ..Default::default()
        },
    ));
    let inherited_unverified = actual_client(&tls_cfg()).unwrap();
    set_outbound_global_default(original);
    assert!(!Arc::ptr_eq(&inherited_verified.runtime, &inherited_unverified.runtime));
}

#[tokio::test]
async fn session_limit_is_bounded_and_raii_release_reopens_capacity() {
    let mut cfg = ExtProcConfig::default();
    cfg.grpc_service.target = "session-cap.example:9000".to_string();
    cfg.grpc_service.timeout_ms = 5;
    let client = actual_client(&cfg).unwrap();
    let mut streams = Vec::new();
    for _ in 0..TARGET_SESSION_CAP {
        streams.push(client.open_stream().await.unwrap());
    }
    let Err(saturated) = client.open_stream().await else {
        panic!("session cap must reject excess logical streams");
    };
    assert_eq!(saturated.code(), tonic::Code::ResourceExhausted);
    streams.pop();
    assert!(
        client.open_stream().await.is_ok(),
        "dropping a stream releases its permit"
    );
}

#[tokio::test]
async fn per_message_timeout_includes_blocked_send_and_lazy_open() {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    sender
        .send(ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
        })
        .await
        .unwrap();
    let semaphore = Arc::new(Semaphore::new(1));
    let permit = semaphore.acquire_owned().await.unwrap();
    let mut stream = ExtProcStream {
        sender: Some(sender),
        inbound: InboundState::Opening(Box::pin(std::future::pending())),
        timeout: Duration::from_millis(5),
        _permit: permit,
    };
    let request = ProcessingRequest {
        request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
    };
    let status = stream.exchange(request, false).await.unwrap_err();
    assert_eq!(status.code(), tonic::Code::DeadlineExceeded);
    drop(receiver);
}

#[tokio::test]
async fn observable_unsolicited_verdict_is_rejected_before_send() {
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let (unsolicited_tx, unsolicited_rx) = tokio::sync::mpsc::channel(1);
    unsolicited_tx.send(Ok(ProcessingResponse::default())).await.unwrap();
    let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
    let mut stream = ExtProcStream {
        sender: Some(sender),
        inbound: InboundState::Test(unsolicited_rx),
        timeout: Duration::from_millis(5),
        _permit: permit,
    };
    let status = stream.reject_observable_unsolicited().unwrap_err();
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(status.message(), "ext_proc processor returned an unsolicited verdict");
}

/// Deterministic channel-backed tests for the request-scoped stream state
/// machine. Real gRPC transport coverage belongs to the integration suite.
mod stream_state {
    use super::*;
    use edgion_common::ext_proc::proto::{
        processing_request, BodyResponse, HeadersResponse, HttpHeaders, HttpTrailers, TrailersResponse,
    };

    #[derive(Clone, Copy)]
    enum TerminalBehavior {
        Clean,
        ExtraVerdict,
        Status,
        NeverClose,
    }

    #[derive(Clone, Copy)]
    enum TerminalPhase {
        ResponseBody,
        ResponseTrailers,
    }

    async fn channel_stream(
        timeout: Duration,
    ) -> (
        ExtProcStream,
        tokio::sync::mpsc::Receiver<ProcessingRequest>,
        tokio::sync::mpsc::Sender<Result<ProcessingResponse, tonic::Status>>,
    ) {
        let (request_tx, request_rx) = tokio::sync::mpsc::channel(4);
        let (response_tx, response_rx) = tokio::sync::mpsc::channel(4);
        let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
        (
            ExtProcStream {
                sender: Some(request_tx),
                inbound: InboundState::Test(response_rx),
                timeout,
                _permit: permit,
            },
            request_rx,
            response_tx,
        )
    }

    async fn spawn_echo_stream() -> ExtProcStream {
        let (stream, mut requests, responses) = channel_stream(Duration::from_secs(2)).await;
        tokio::spawn(async move {
            let mut rank = 0u8;
            while let Some(message) = requests.recv().await {
                let (next_rank, response) = match message.request {
                    Some(processing_request::Request::RequestHeaders(_)) => (
                        1,
                        processing_response::Response::RequestHeaders(HeadersResponse::default()),
                    ),
                    Some(processing_request::Request::RequestBody(_)) => {
                        (2, processing_response::Response::RequestBody(BodyResponse::default()))
                    }
                    Some(processing_request::Request::ResponseHeaders(_)) => (
                        3,
                        processing_response::Response::ResponseHeaders(HeadersResponse::default()),
                    ),
                    Some(processing_request::Request::ResponseBody(_)) => {
                        (4, processing_response::Response::ResponseBody(BodyResponse::default()))
                    }
                    Some(processing_request::Request::ResponseTrailers(_)) => (
                        5,
                        processing_response::Response::ResponseTrailers(TrailersResponse::default()),
                    ),
                    _ => break,
                };
                if next_rank < rank {
                    let _ = responses
                        .send(Err(tonic::Status::failed_precondition("phase order regressed")))
                        .await;
                    break;
                }
                rank = next_rank;
                if responses
                    .send(Ok(ProcessingResponse {
                        response: Some(response),
                        mode_override: None,
                    }))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        stream
    }

    async fn spawn_terminal_stream(
        behavior: TerminalBehavior,
        phase: TerminalPhase,
    ) -> (ExtProcStream, tokio::sync::oneshot::Receiver<bool>) {
        let timeout = if matches!(behavior, TerminalBehavior::NeverClose) {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(2)
        };
        let (stream, mut requests, responses) = channel_stream(timeout).await;
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let first = requests.recv().await;
            let matches_phase = matches!(
                (phase, first),
                (
                    TerminalPhase::ResponseBody,
                    Some(ProcessingRequest {
                        request: Some(processing_request::Request::ResponseBody(_)),
                    })
                ) | (
                    TerminalPhase::ResponseTrailers,
                    Some(ProcessingRequest {
                        request: Some(processing_request::Request::ResponseTrailers(_)),
                    })
                )
            );
            if !matches_phase {
                return;
            }
            let matching_response = match phase {
                TerminalPhase::ResponseBody => processing_response::Response::ResponseBody(BodyResponse::default()),
                TerminalPhase::ResponseTrailers => {
                    processing_response::Response::ResponseTrailers(TrailersResponse::default())
                }
            };
            if responses
                .send(Ok(ProcessingResponse {
                    response: Some(matching_response.clone()),
                    mode_override: None,
                }))
                .await
                .is_err()
            {
                return;
            }
            let clean_eof = requests.recv().await.is_none();
            let _ = observed_tx.send(clean_eof);
            match behavior {
                TerminalBehavior::Clean => {}
                TerminalBehavior::ExtraVerdict => {
                    let _ = responses
                        .send(Ok(ProcessingResponse {
                            response: Some(matching_response),
                            mode_override: None,
                        }))
                        .await;
                }
                TerminalBehavior::Status => {
                    let _ = responses
                        .send(Err(tonic::Status::resource_exhausted("terminal status detail")))
                        .await;
                }
                TerminalBehavior::NeverClose => std::future::pending::<()>().await,
            }
        });
        (stream, observed_rx)
    }

    /// One stream carries the header verdict plus several body-chunk
    /// verdicts, with the final chunk half-closing the outbound side.
    #[tokio::test]
    async fn stream_carries_headers_and_multiple_body_chunks() {
        let mut stream = spawn_echo_stream().await;

        let headers = ProcessingRequest {
            request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                headers: None,
                end_of_stream: false,
            })),
        };
        let verdict = stream.exchange(headers, false).await.unwrap();
        assert!(matches!(
            verdict.response,
            Some(processing_response::Response::RequestHeaders(_))
        ));

        for (index, last) in [(0u8, false), (1, false), (2, true)] {
            let chunk = ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(HttpBody {
                    body: vec![index],
                    end_of_stream: last,
                })),
            };
            let verdict = stream.exchange(chunk, last).await.unwrap();
            assert!(matches!(
                verdict.response,
                Some(processing_response::Response::RequestBody(_))
            ));
        }
    }

    #[tokio::test]
    async fn one_process_stream_carries_request_and_response_in_exact_order() {
        let mut stream = spawn_echo_stream().await;
        let messages = vec![
            ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
            },
            ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(HttpBody {
                    body: b"request".to_vec(),
                    end_of_stream: true,
                })),
            },
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseHeaders(HttpHeaders::default())),
            },
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(HttpBody {
                    body: b"response".to_vec(),
                    end_of_stream: true,
                })),
            },
        ];
        for (index, message) in messages.into_iter().enumerate() {
            stream.exchange(message, index == 3).await.unwrap();
        }
    }

    #[tokio::test]
    async fn streamed_response_then_trailers_is_ordered_and_half_closes_on_trailers() {
        let mut stream = spawn_echo_stream().await;

        for message in [
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseHeaders(HttpHeaders {
                    headers: None,
                    end_of_stream: false,
                })),
            },
            ProcessingRequest {
                request: Some(processing_request::Request::ResponseBody(HttpBody {
                    body: b"response".to_vec(),
                    end_of_stream: false,
                })),
            },
        ] {
            stream.exchange(message, false).await.unwrap();
        }
        let verdict = stream
            .exchange(
                ProcessingRequest {
                    request: Some(processing_request::Request::ResponseTrailers(HttpTrailers::default())),
                },
                true,
            )
            .await
            .unwrap();
        assert!(matches!(
            verdict.response,
            Some(processing_response::Response::ResponseTrailers(_))
        ));
    }

    #[tokio::test]
    async fn final_response_body_and_trailers_close_request_channel() {
        for phase in [TerminalPhase::ResponseBody, TerminalPhase::ResponseTrailers] {
            let (mut stream, observed) = spawn_terminal_stream(TerminalBehavior::Clean, phase).await;
            let request = match phase {
                TerminalPhase::ResponseBody => processing_request::Request::ResponseBody(HttpBody {
                    body: b"final".to_vec(),
                    end_of_stream: true,
                }),
                TerminalPhase::ResponseTrailers => {
                    processing_request::Request::ResponseTrailers(HttpTrailers::default())
                }
            };
            stream
                .exchange(ProcessingRequest { request: Some(request) }, true)
                .await
                .unwrap();
            assert!(observed.await.expect("processor task reports request-channel state"));
        }
    }

    #[tokio::test]
    async fn terminal_drain_rejects_extra_verdict_and_timeout() {
        for (behavior, expected) in [
            (TerminalBehavior::ExtraVerdict, tonic::Code::FailedPrecondition),
            (TerminalBehavior::Status, tonic::Code::ResourceExhausted),
            (TerminalBehavior::NeverClose, tonic::Code::DeadlineExceeded),
        ] {
            let (mut stream, observed) = spawn_terminal_stream(behavior, TerminalPhase::ResponseTrailers).await;
            let error = stream
                .exchange(
                    ProcessingRequest {
                        request: Some(processing_request::Request::ResponseTrailers(HttpTrailers::default())),
                    },
                    true,
                )
                .await
                .unwrap_err();
            assert_eq!(error.code(), expected);
            assert!(observed.await.expect("processor task reports request-channel state"));
        }
    }

    /// A message sent after the outbound half-close must fail cleanly, not
    /// hang: the sender was consumed by `last = true`.
    #[tokio::test]
    async fn exchange_after_half_close_fails() {
        let mut stream = spawn_echo_stream().await;

        let eos_chunk = ProcessingRequest {
            request: Some(processing_request::Request::RequestBody(HttpBody {
                body: Vec::new(),
                end_of_stream: true,
            })),
        };
        stream.exchange(eos_chunk.clone(), true).await.unwrap();
        let err = stream.exchange(eos_chunk, true).await.unwrap_err();
        assert!(matches!(err.code(), tonic::Code::Internal | tonic::Code::Aborted));
    }
}

#[test]
fn response_status_detects_continue_and_replace_and_rejects_unknown() {
    use edgion_common::ext_proc::proto::{HeadersResponse, ProcessingResponse};

    let response_with_status = |status| ProcessingResponse {
        response: Some(processing_response::Response::RequestHeaders(HeadersResponse {
            response: Some(CommonResponse {
                status,
                ..Default::default()
            }),
        })),
        mode_override: None,
    };

    assert_eq!(
        response_status(&response_with_status(ResponseStatus::ContinueAndReplace as i32)).unwrap(),
        ResponseStatus::ContinueAndReplace
    );
    assert_eq!(
        response_status(&response_with_status(ResponseStatus::Continue as i32)).unwrap(),
        ResponseStatus::Continue
    );
    assert_eq!(
        response_status(&response_with_status(99)).unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
}

#[tokio::test]
async fn final_phase_half_closes_request_stream() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut sender = Some(tx);
    let request = ProcessingRequest {
        request: Some(processing_request::Request::RequestHeaders(HttpHeaders::default())),
    };

    send_phase(&mut sender, request, true).await.unwrap();
    assert!(rx.recv().await.is_some());
    assert!(
        rx.recv().await.is_none(),
        "the processor must observe EOF immediately after the final phase"
    );
}

/// System CA + verify=true (default outbound) + tls=true must construct a
/// channel. `connect_with_connector_lazy` never touches the network, so this
/// proves the connector output type satisfies tonic's IO + `Connection`
/// bounds and the whole TLS path type-checks.
#[tokio::test]
async fn extproc_connector_built_for_system_verify() {
    install_test_crypto_provider();
    let cfg = tls_cfg();
    assert!(actual_client(&cfg).is_ok());
}

/// The plain-HTTP path (tls=false) must keep building a lazy channel exactly
/// as before.
#[tokio::test]
async fn extproc_plain_http_channel_builds() {
    let mut cfg = tls_cfg();
    cfg.grpc_service.tls.enabled = false;
    assert!(actual_client(&cfg).is_ok());
}

/// verify=false must build the AcceptAllVerifier branch without error —
/// the operator's choice is honored regardless of environment.
#[tokio::test]
async fn extproc_verify_false_builds() {
    install_test_crypto_provider();

    let mut cfg = tls_cfg();
    cfg.grpc_service.tls.outbound = OutboundTlsLocal {
        verify: Some(false),
        ..Default::default()
    };
    let result = actual_client(&cfg);

    assert!(
        result.is_ok(),
        "verify=false must build the AcceptAllVerifier channel, got: {:?}",
        result.err()
    );
}

/// Custom CA via resolved_ca_certificates: supply a valid self-signed CA PEM in a K8s Secret;
/// the resolver must build the REPLACE-root ClientConfig without error.
#[tokio::test]
async fn extproc_custom_inline_ca_builds() {
    install_test_crypto_provider();

    let ca = rcgen::generate_simple_self_signed(vec!["test-ca".into()]).unwrap();
    let ca_pem = ca.cert.pem().into_bytes();

    // Build a K8s Secret carrying the CA PEM under the expected key.
    let mut data = std::collections::BTreeMap::new();
    data.insert("ca.crt".to_string(), k8s_openapi::ByteString(ca_pem));
    let ca_secret = k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("test-ca-secret".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    };

    let mut cfg = tls_cfg();
    // Declare a caCertificateRef so the resolver picks up the custom CA.
    cfg.grpc_service.tls.outbound = OutboundTlsLocal {
        validation: Some(OutboundTlsValidation {
            ca_certificate_refs: Some(vec![CustomCACertificateRef {
                kind: "Secret".to_string(),
                name: "test-ca-secret".to_string(),
                group: Some(String::new()),
                namespace: None,
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    cfg.grpc_service.tls.resolved_ca_certificates = Some(vec![ca_secret]);

    assert!(
        actual_client(&cfg).is_ok(),
        "Custom CA via resolved_ca_certificates must build the REPLACE-root channel without error"
    );
}

fn bound_tls_owner() -> (edgion_resources::EdgionPlugins, Vec<u8>, Vec<u8>) {
    let generated = rcgen::generate_simple_self_signed(vec!["processor.example".into()]).unwrap();
    let pem = generated.cert.pem().into_bytes();
    let key = generated.signing_key.serialize_pem().into_bytes();
    let secret = |name: &str, data: Vec<(&str, Vec<u8>)>| k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            resource_version: Some("7".into()),
            ..Default::default()
        },
        data: Some(
            data.into_iter()
                .map(|(k, v)| (k.into(), k8s_openapi::ByteString(v)))
                .collect(),
        ),
        ..Default::default()
    };
    let owner = serde_json::from_value(serde_json::json!({"metadata":{"name":"processor","uid":"owner"},"spec":{"requestPlugins":[{"type":"ExtProc","config":{
        "grpcService":{"target":"bound-processor.example:9000","timeoutMs":200,"tls":{"enabled":true,"verify":true,
        "validation":{"caCertificateRefs":[{"kind":"Secret","group":"core","name":"ca"}]},"clientCertificateRef":{"group":"core","name":"client"},
        "resolvedCaCertificates":[secret("ca",vec![("ca.crt",pem.clone())])],"resolvedClientCertificate":secret("client",vec![("tls.crt",pem.clone()),("tls.key",key.clone())])}}
    }}]}})).unwrap();
    (owner, pem, key)
}

#[tokio::test]
async fn actual_tls_identity_is_checked_before_target_cache_and_valid_reuse_is_preserved() {
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
    let (owner, pem, key) = bound_tls_owner();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let first = ExtProcClient::new(&entry, &defaults).unwrap();
    let base = serde_json::to_value(&owner).unwrap();
    for case in [
        "matching",
        "configmap",
        "owner",
        "owner-missing",
        "owner-ns",
        "ca-name",
        "ca-missing-name",
        "ca-ns",
        "ca-missing-ns",
        "ca-empty-ns",
        "client-name",
        "client-missing-name",
        "client-ns",
        "client-missing-ns",
        "client-empty-ns",
        "source",
        "client-source",
        "retarget",
        "no-ref",
        "no-ca",
        "no-client",
    ] {
        let mut candidate = base.clone();
        match case {
            "owner" => candidate["metadata"]["name"] = serde_json::json!("bad/name"),
            "owner-missing" => candidate["metadata"]["name"] = serde_json::Value::Null,
            "owner-ns" => candidate["metadata"]["namespace"] = serde_json::json!(""),
            _ => {}
        }
        let tls = &mut candidate["spec"]["requestPlugins"][0]["config"]["grpcService"]["tls"];
        match case {
            "configmap" => {
                tls["validation"]["caCertificateRefs"][0]["kind"] = serde_json::json!("ConfigMap");
                tls["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"});
            }
            "ca-name" => tls["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::json!("other"),
            "ca-missing-name" => tls["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::Value::Null,
            "ca-ns" => tls["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!("other"),
            "ca-missing-ns" => tls["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::Value::Null,
            "ca-empty-ns" => tls["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!(""),
            "client-name" => tls["resolvedClientCertificate"]["metadata"]["name"] = serde_json::json!("other"),
            "client-missing-name" => tls["resolvedClientCertificate"]["metadata"]["name"] = serde_json::Value::Null,
            "client-ns" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!("other"),
            "client-missing-ns" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::Value::Null,
            "client-empty-ns" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!(""),
            "source" => {
                tls["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
            }
            "client-source" => {
                tls["resolvedClientCertificate"]["metadata"]["annotations"] =
                    serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
            }
            "retarget" => tls["validation"]["caCertificateRefs"][0]["name"] = serde_json::json!("next"),
            "no-ref" => tls["validation"]["caCertificateRefs"] = serde_json::Value::Null,
            "no-ca" => tls["resolvedCaCertificates"] = serde_json::Value::Null,
            "no-client" => tls["resolvedClientCertificate"] = serde_json::Value::Null,
            _ => {}
        }
        let candidate: edgion_resources::EdgionPlugins = serde_json::from_value(candidate).unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&candidate);
        let binding = prepared.bind(&candidate).unwrap();
        let entry = binding.request_entries().next().unwrap();
        if matches!(case, "matching" | "configmap") {
            let proof = ExtProcClient::validate_tls_entry(&entry, &defaults).unwrap().unwrap();
            let resolved = resolve_validated_outbound_tls(&proof, true, &defaults).unwrap();
            let EffectiveCa::Custom(bundles) = resolved.ca else {
                panic!("actual CA required")
            };
            assert_eq!(bundles, vec![pem.clone()]);
            let (cert, private_key) = resolved.client_identity.unwrap();
            assert_eq!(cert, pem);
            assert_eq!(private_key.as_slice(), key.as_slice());
            let next = ExtProcClient::new(&entry, &defaults).unwrap();
            assert_eq!(Arc::ptr_eq(&first.runtime, &next.runtime), case == "matching");
        } else {
            assert_eq!(
                ExtProcClient::validate_tls_entry(&entry, &defaults).err().as_deref(),
                Some("ext_proc TLS configuration is invalid"),
                "{case}"
            );
            assert_eq!(
                ExtProcClient::new(&entry, &defaults).err().as_deref(),
                Some("ext_proc TLS configuration is invalid"),
                "{case}"
            );
        }
    }
    for case in ["owner-only", "retarget", "rotation", "restoration"] {
        let mut candidate = base.clone();
        candidate["metadata"]["name"] = serde_json::json!("another-owner");
        candidate["spec"]["requestPlugins"][0]["config"]["grpcService"]["timeoutMs"] = serde_json::json!(901);
        let tls = &mut candidate["spec"]["requestPlugins"][0]["config"]["grpcService"]["tls"];
        if case == "retarget" {
            tls["validation"]["caCertificateRefs"][0]["name"] = serde_json::json!("next");
            tls["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::json!("next");
        }
        if case == "rotation" {
            let next = rcgen::generate_simple_self_signed(vec!["rotated.example".into()]).unwrap();
            tls["resolvedCaCertificates"][0]["data"]["ca.crt"] =
                serde_json::to_value(k8s_openapi::ByteString(next.cert.pem().into_bytes())).unwrap();
        }
        let candidate: edgion_resources::EdgionPlugins = serde_json::from_value(candidate).unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&candidate);
        let binding = prepared.bind(&candidate).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let next = ExtProcClient::new(&entry, &defaults).unwrap();
        assert_eq!(
            Arc::ptr_eq(&first.runtime, &next.runtime),
            matches!(case, "owner-only" | "restoration")
        );
        assert_eq!(next.timeout, Duration::from_millis(901));
    }
    assert_eq!(first.timeout, Duration::from_millis(200));
}

#[tokio::test]
async fn captured_tls_generation_and_plaintext_inactivity_are_process_isolated() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::ext_proc::client::tests",
        "captured_tls_generation_and_plaintext_inactivity_are_process_isolated",
        "EDGION_EXTPROC_TLS_CAPTURE_CHILD",
        "captured",
        || async {
            use edgion_common::link_sys_core::outbound_tls::{
                outbound_global_default, set_outbound_global_default, test_util::snapshot,
            };
            use edgion_resources::resources::common::OutboundTlsConfig;
            let previous = outbound_global_default();
            let (mut owner, pem, key) = bound_tls_owner();
            let EdgionPlugin::ExtProc(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            let tls = &mut config.grpc_service.tls;
            let mut global = OutboundTlsConfig {
                verify: true,
                validation: tls.outbound.validation.take(),
                client_certificate_ref: tls.outbound.client_certificate_ref.take(),
                resolved_ca_certificates: tls.resolved_ca_certificates.take(),
                resolved_client_certificate: tls.resolved_client_certificate.take(),
            };
            global.client_certificate_ref.as_mut().unwrap().namespace = Some("default".into());
            for ca in global
                .validation
                .as_mut()
                .unwrap()
                .ca_certificate_refs
                .as_mut()
                .unwrap()
            {
                ca.namespace = Some("default".into());
            }
            let captured = snapshot(global.clone());
            global.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("wrong".into());
            let invalid = snapshot(global);
            set_outbound_global_default(invalid.clone());
            let prepared = PreparedPluginReferences::from_plugins(&owner);
            let binding = prepared.bind(&owner).unwrap();
            let entry = binding.request_entries().next().unwrap();
            let proof = ExtProcClient::validate_tls_entry(&entry, &captured).unwrap().unwrap();
            let resolved = resolve_validated_outbound_tls(&proof, true, &captured).unwrap();
            let EffectiveCa::Custom(bundles) = resolved.ca else {
                panic!("captured CA")
            };
            assert_eq!(bundles, vec![pem.clone()]);
            let (cert, private_key) = resolved.client_identity.unwrap();
            assert_eq!(cert, pem);
            assert_eq!(private_key.as_slice(), key.as_slice());
            assert!(ExtProcClient::new(&entry, &captured).is_ok());
            set_outbound_global_default(captured);
            assert!(ExtProcClient::new(&entry, &invalid).is_err());
            let (mut local, _, _) = bound_tls_owner();
            {
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                assert!(ExtProcClient::new(&binding.request_entries().next().unwrap(), &invalid).is_ok());
            }
            let EdgionPlugin::ExtProc(config) = &mut local.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            config.grpc_service.tls.enabled = false;
            config.grpc_service.tls.outbound.validation.as_mut().unwrap().hostname = Some("unsupported.example".into());
            config
                .grpc_service
                .tls
                .resolved_client_certificate
                .as_mut()
                .unwrap()
                .metadata
                .name = None;
            {
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                let entry = binding.request_entries().next().unwrap();
                assert!(ExtProcClient::validate_tls_entry(&entry, &invalid).unwrap().is_none());
                assert!(ExtProcClient::new(&entry, &invalid).is_ok());
            }
            local.metadata.name = None;
            let prepared = PreparedPluginReferences::from_plugins(&local);
            let binding = prepared.bind(&local).unwrap();
            assert!(ExtProcClient::new(&binding.request_entries().next().unwrap(), &invalid).is_err());
            set_outbound_global_default(previous);
        },
    )
    .await;
}
