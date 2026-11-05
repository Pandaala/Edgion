//! Canonical metric classes, kinds and label domains. Resource identity is never fixed.
use metrics::Key;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricClass {
    Fixed,
    Dynamic,
    Snapshot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
}
#[derive(Clone, Copy, Debug)]
pub enum LabelDomain {
    Closed(&'static [&'static str]),
    Kind,
    Open,
}
#[derive(Clone, Copy, Debug)]
pub struct LabelDefinition {
    pub key: &'static str,
    pub domain: LabelDomain,
    pub required: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct Definition {
    pub name: &'static str,
    pub kind: MetricType,
    pub class: MetricClass,
    pub labels: &'static [LabelDefinition],
    pub contribution: bool,
}

/// Limits apply to prepared metric identities, not business configuration acceptance.
pub const MAX_LABEL_BYTES: usize = 1024;
pub const MAX_KEY_BYTES: usize = 4096;
pub const MAX_LABELS: usize = 18;

pub static DEFINITIONS: &[Definition] = &[
    Definition {
        name: "edgion_ctx_created_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_ctx_active",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_access_log_dropped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_ssl_log_dropped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_tcp_log_dropped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_tls_log_dropped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_udp_log_dropped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_access_log_backfill_ingested_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_access_log_backfill_failed_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_config_reload_signals_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_config_relist_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_config_event_parse_errors_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Kind,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_compile_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "result",
            domain: LabelDomain::Closed(&["ok", "error"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_compile_microseconds_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "result",
            domain: LabelDomain::Closed(&["ok", "error"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_generation_bytes",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_runtime_bytes_active",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_body_bytes_inspected_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "direction",
            domain: LabelDomain::Closed(&["request", "response"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_execution_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "phase",
            domain: LabelDomain::Closed(&[
                "request_headers",
                "request_body",
                "response_headers",
                "response_body",
                "logging",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_waf_execution_microseconds_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "phase",
            domain: LabelDomain::Closed(&[
                "request_headers",
                "request_body",
                "response_headers",
                "response_body",
                "logging",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_request_body_storage_io_failures_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "operation",
                domain: LabelDomain::Closed(&[
                    "spill_create",
                    "spill_write",
                    "replay_read",
                    "materialize_read",
                    "snapshot_read",
                ]),
                required: true,
            },
            LabelDefinition {
                key: "reason",
                domain: LabelDomain::Closed(&[
                    "admission_timeout",
                    "admission_closed",
                    "execution_timeout",
                    "join_error",
                    "io_error",
                ]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_request_body_storage_io_inflight",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_link_sys_es_bulk_channel_full_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_link_sys_health",
        kind: MetricType::Gauge,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "link",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_link_sys_health_flips_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "link",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "direction",
                domain: LabelDomain::Closed(&["recovered", "degraded"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_ldap_auth_errors_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Closed(&["unreachable", "timeout", "bind_fail", "tls_san_mismatch"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_outbound_tls_san_mismatch_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "plugin",
            domain: LabelDomain::Closed(&["ldap_auth", "openid_connect"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_webhook_resolve_errors_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "reason",
            domain: LabelDomain::Closed(&[
                "not_found",
                "unhealthy",
                "rate_limited",
                "status_error",
                "call_failed",
                "authority_rejected",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_outbound_egress_blocked_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "reason",
            domain: LabelDomain::Closed(&["loopback", "link_local", "private", "unspecified"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_upstream_authority_rejected_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "reason",
            domain: LabelDomain::Closed(&["render_empty", "invalid_label"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_outbound_success_predicate_fail_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_outbound_template_fallback_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "field",
            domain: LabelDomain::Closed(&["timeout_ms"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_config_conflict_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "field",
            domain: LabelDomain::Closed(&["outbound_ca", "outbound_client_identity"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_outbound_injected_header_skipped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_fips_module_enabled",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_fips_host_enabled",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_fips_host_detect_error",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_fips_policy_active",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "module_enabled",
            domain: LabelDomain::Closed(&["true", "false"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_module_load_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "module",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "result",
                domain: LabelDomain::Closed(&["ok", "error"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_module_compile_duration_ms",
        kind: MetricType::Histogram,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "module",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_module_released_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "module",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_instance_instantiate_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "module",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_instance_discard_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "module",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "reason",
                domain: LabelDomain::Closed(&["poisoned", "over_capacity"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_pool_size",
        kind: MetricType::Gauge,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "module",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: true,
    },
    Definition {
        name: "edgion_wasm_hook_duration_ms",
        kind: MetricType::Histogram,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "module",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "phase",
                domain: LabelDomain::Closed(&[
                    "request_headers",
                    "request_body",
                    "response_headers",
                    "response_body",
                    "http_call_response",
                    "tick",
                ]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_trap_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "module",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "phase",
                domain: LabelDomain::Open,
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_fail_mode_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "module",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "reason",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "mode",
                domain: LabelDomain::Closed(&["open", "close"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_callout_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "module",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "result",
                domain: LabelDomain::Closed(&["ok", "error", "denied", "over_limit"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_modules_loaded",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_wasm_shared_data_bytes",
        kind: MetricType::Gauge,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "module",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: true,
    },
    Definition {
        name: "edgion_wasm_body_pause_unsupported_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "module",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_plugin_policy_blocked_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "plugin",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_plugin_policy_bypassed_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "plugin",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_geoip_db_loaded",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_geoip_db_build_epoch_seconds",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_geoip_update_failed_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "reason",
            domain: LabelDomain::Closed(&[
                "fetch",
                "encoding",
                "size_cap",
                "stall",
                "validation",
                "rename",
                "disk",
                "deadline",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_geoip_lookup_panic_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_retry_budget_denied_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_conf_sync_cert_expiry_timestamp_seconds",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "side",
            domain: LabelDomain::Closed(&["server", "client"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_system_error_state",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "component",
                domain: LabelDomain::Closed(&[
                    "conf_sync_client",
                    "conf_sync_server",
                    "fed_sync_client",
                    "admin_api",
                    "gateway_runtime",
                ]),
                required: true,
            },
            LabelDefinition {
                key: "reason",
                domain: LabelDomain::Closed(&[
                    "skip_tls",
                    "no_tls_configured",
                    "peer_identity_self_check",
                    "insecure_admin_exposure",
                    "integration_testing_mode",
                    "confsync_disconnected",
                    "cert_rotation_config_suspect",
                    "cert_reload_invalid",
                    "cert_rotation_failed",
                    "cert_identity_sync_failed",
                    "cert_watch_failed",
                ]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_connections_active",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "role",
            domain: LabelDomain::Closed(&["center", "controller"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_connection_events_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "role",
                domain: LabelDomain::Closed(&["center", "controller"]),
                required: true,
            },
            LabelDefinition {
                key: "event",
                domain: LabelDomain::Closed(&["connected", "disconnected", "reload"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_connection_duration_seconds_last",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "role",
            domain: LabelDomain::Closed(&["center", "controller"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_watch_events_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "kind",
                domain: LabelDomain::Kind,
                required: true,
            },
            LabelDefinition {
                key: "direction",
                domain: LabelDomain::Closed(&["sent", "recv"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_ready_gate_wait_seconds_last",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_rbac_denied_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "verb",
                domain: LabelDomain::Closed(&[
                    "access-self",
                    "get",
                    "list",
                    "list-keys",
                    "watch",
                    "create",
                    "update",
                    "delete",
                    "failover",
                    "acme-trigger",
                    "conf-sync-rotate",
                    "reload",
                    "server-info",
                    "diagnostics",
                    "wipe-all",
                    "debug",
                    "unknown",
                ]),
                required: true,
            },
            LabelDefinition {
                key: "kind",
                domain: LabelDomain::Kind,
                required: true,
            },
            LabelDefinition {
                key: "source",
                domain: LabelDomain::Closed(&["center", "cli_token", "unknown"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_kill_switch_state",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_fed_client_running",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_resource_abnormal",
        kind: MetricType::Gauge,
        class: MetricClass::Snapshot,
        labels: &[
            LabelDefinition {
                key: "kind",
                domain: LabelDomain::Kind,
                required: true,
            },
            LabelDefinition {
                key: "namespace",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "name",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "err",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "warn",
                domain: LabelDomain::Open,
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_unknown_config_fields",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Closed(&["EdgionPlugins", "EdgionStreamPlugins", "EdgionConfigData", "LinkSys"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_init_sync_duration_seconds_last",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Kind,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_active",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_resources_watched",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Kind,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_leader",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_leader_transitions_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_reload_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_reload_duration_seconds_last",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_reload_client_notifications_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_status_writes_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "kind",
                domain: LabelDomain::Kind,
                required: true,
            },
            LabelDefinition {
                key: "backend",
                domain: LabelDomain::Closed(&["kubernetes", "filesystem", "etcd"]),
                required: true,
            },
            LabelDefinition {
                key: "result",
                domain: LabelDomain::Closed(&["success", "error"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_status_writes_skipped_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Kind,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_status_conditions_max",
        kind: MetricType::Gauge,
        class: MetricClass::Snapshot,
        labels: &[
            LabelDefinition {
                key: "kind",
                domain: LabelDomain::Kind,
                required: true,
            },
            LabelDefinition {
                key: "namespace",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "name",
                domain: LabelDomain::Open,
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_unexpected_apply_during_init_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "kind",
            domain: LabelDomain::Kind,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_controller_leader_job_alive",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "leader_job",
            domain: LabelDomain::Closed(&["acme", "conf_sync_rotation", "credential_manager", "dns_resolver"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_metrics_series_rejected_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_metrics_cardinality_limit_active",
        kind: MetricType::Gauge,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_backend_requests_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "gateway_namespace",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "gateway_name",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "route_namespace",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "route_name",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "backend_namespace",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "backend_name",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "protocol",
                domain: LabelDomain::Closed(&["http", "grpc", "websocket", "grpc-web"]),
                required: false,
            },
            LabelDefinition {
                key: "status",
                domain: LabelDomain::Closed(&["2xx", "3xx", "4xx", "5xx", "failed"]),
                required: false,
            },
            LabelDefinition {
                key: "test_key",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "test_data",
                domain: LabelDomain::Open,
                required: false,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_request_duration_ms",
        kind: MetricType::Histogram,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "gateway_namespace",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "gateway_name",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "route_namespace",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "route_name",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "backend_namespace",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "backend_name",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "protocol",
                domain: LabelDomain::Closed(&["http", "grpc", "websocket", "grpc-web"]),
                required: false,
            },
            LabelDefinition {
                key: "status",
                domain: LabelDomain::Closed(&["2xx", "3xx", "4xx", "5xx", "failed"]),
                required: false,
            },
            LabelDefinition {
                key: "test_key",
                domain: LabelDomain::Open,
                required: false,
            },
            LabelDefinition {
                key: "test_data",
                domain: LabelDomain::Open,
                required: false,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_request_bytes_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_response_bytes_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_genai_tokens_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "direction",
                domain: LabelDomain::Closed(&["input", "output"]),
                required: true,
            },
            LabelDefinition {
                key: "provider",
                domain: LabelDomain::Closed(&["openai", "openai_compatible", "anthropic"]),
                required: true,
            },
            LabelDefinition {
                key: "usage_source",
                domain: LabelDomain::Closed(&["reported", "estimated"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_mirror_requests_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "status",
            domain: LabelDomain::Closed(&[
                "ok",
                "body_abandoned",
                "channel_full",
                "buffer_limit",
                "connect_err",
                "connect_timeout",
                "write_err",
                "timeout",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_conn_ip_restriction_events_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "result",
            domain: LabelDomain::Closed(&[
                "overlay_missing",
                "overlay_disabled",
                "overlay_type_mismatch",
                "overlay_invalid",
                "fail_closed_profile_missing",
                "denied",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_geoip_denied_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "layer",
                domain: LabelDomain::Closed(&["stage1", "tcp"]),
                required: true,
            },
            LabelDefinition {
                key: "reason",
                domain: LabelDomain::Closed(&[
                    "deny_country",
                    "allow_miss",
                    "db_unavailable",
                    "lookup_failed",
                    "ref_unavailable",
                ]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_connection_rate_limit_rejected_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "dimension",
                domain: LabelDomain::Closed(&["listener", "source_ip"]),
                required: true,
            },
            LabelDefinition {
                key: "stage",
                domain: LabelDomain::Closed(&["local", "redis"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_connection_rate_limit_redis_failure_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[
            LabelDefinition {
                key: "dimension",
                domain: LabelDomain::Closed(&["listener", "source_ip"]),
                required: true,
            },
            LabelDefinition {
                key: "cause",
                domain: LabelDomain::Closed(&["unavailable", "runtime"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_circuit_breaker_ejections_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "service",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "class",
                domain: LabelDomain::Closed(&["gateway", "application", "local_origin"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_circuit_breaker_ejected_endpoints",
        kind: MetricType::Gauge,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "service",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: true,
    },
    Definition {
        name: "edgion_circuit_breaker_rejections_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "service",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_gateway_circuit_breaker_shed_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "service",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_lb_degrade_selections_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[LabelDefinition {
            key: "service",
            domain: LabelDomain::Open,
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_rr_selector_truncated_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[],
        contribution: false,
    },
    Definition {
        name: "edgion_region_route_overlay_events_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "result",
            domain: LabelDomain::Closed(&[
                "overlay_missing",
                "overlay_disabled",
                "overlay_type_mismatch",
                "overlay_invalid",
            ]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_acme_challenge_lookup_total",
        kind: MetricType::Counter,
        class: MetricClass::Fixed,
        labels: &[LabelDefinition {
            key: "result",
            domain: LabelDomain::Closed(&["hit", "expired", "domain_mismatch", "not_found", "empty_host"]),
            required: true,
        }],
        contribution: false,
    },
    Definition {
        name: "edgion_link_sys_redis_connections",
        kind: MetricType::Gauge,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "link",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "state",
                domain: LabelDomain::Closed(&["live", "configured"]),
                required: true,
            },
        ],
        contribution: false,
    },
    Definition {
        name: "edgion_link_sys_redis_topology_events_total",
        kind: MetricType::Counter,
        class: MetricClass::Dynamic,
        labels: &[
            LabelDefinition {
                key: "link",
                domain: LabelDomain::Open,
                required: true,
            },
            LabelDefinition {
                key: "event",
                domain: LabelDomain::Closed(&["recovery_failed", "recovered", "primary_changed"]),
                required: true,
            },
        ],
        contribution: false,
    },
];

pub fn definition(name: &str) -> Option<&'static Definition> {
    match name {
        "edgion_ctx_created_total" => Some(&DEFINITIONS[0]),
        "edgion_ctx_active" => Some(&DEFINITIONS[1]),
        "edgion_access_log_dropped_total" => Some(&DEFINITIONS[2]),
        "edgion_ssl_log_dropped_total" => Some(&DEFINITIONS[3]),
        "edgion_tcp_log_dropped_total" => Some(&DEFINITIONS[4]),
        "edgion_tls_log_dropped_total" => Some(&DEFINITIONS[5]),
        "edgion_udp_log_dropped_total" => Some(&DEFINITIONS[6]),
        "edgion_access_log_backfill_ingested_total" => Some(&DEFINITIONS[7]),
        "edgion_access_log_backfill_failed_total" => Some(&DEFINITIONS[8]),
        "edgion_config_reload_signals_total" => Some(&DEFINITIONS[9]),
        "edgion_config_relist_total" => Some(&DEFINITIONS[10]),
        "edgion_config_event_parse_errors_total" => Some(&DEFINITIONS[11]),
        "edgion_gateway_waf_compile_total" => Some(&DEFINITIONS[12]),
        "edgion_gateway_waf_compile_microseconds_total" => Some(&DEFINITIONS[13]),
        "edgion_gateway_waf_generation_bytes" => Some(&DEFINITIONS[14]),
        "edgion_gateway_waf_runtime_bytes_active" => Some(&DEFINITIONS[15]),
        "edgion_gateway_waf_body_bytes_inspected_total" => Some(&DEFINITIONS[16]),
        "edgion_gateway_waf_execution_total" => Some(&DEFINITIONS[17]),
        "edgion_gateway_waf_execution_microseconds_total" => Some(&DEFINITIONS[18]),
        "edgion_request_body_storage_io_failures_total" => Some(&DEFINITIONS[19]),
        "edgion_request_body_storage_io_inflight" => Some(&DEFINITIONS[20]),
        "edgion_link_sys_es_bulk_channel_full_total" => Some(&DEFINITIONS[21]),
        "edgion_link_sys_health" => Some(&DEFINITIONS[22]),
        "edgion_link_sys_health_flips_total" => Some(&DEFINITIONS[23]),
        "edgion_ldap_auth_errors_total" => Some(&DEFINITIONS[24]),
        "edgion_gateway_outbound_tls_san_mismatch_total" => Some(&DEFINITIONS[25]),
        "edgion_webhook_resolve_errors_total" => Some(&DEFINITIONS[26]),
        "edgion_outbound_egress_blocked_total" => Some(&DEFINITIONS[27]),
        "edgion_gateway_upstream_authority_rejected_total" => Some(&DEFINITIONS[28]),
        "edgion_outbound_success_predicate_fail_total" => Some(&DEFINITIONS[29]),
        "edgion_outbound_template_fallback_total" => Some(&DEFINITIONS[30]),
        "edgion_config_conflict_total" => Some(&DEFINITIONS[31]),
        "edgion_outbound_injected_header_skipped_total" => Some(&DEFINITIONS[32]),
        "edgion_gateway_fips_module_enabled" => Some(&DEFINITIONS[33]),
        "edgion_gateway_fips_host_enabled" => Some(&DEFINITIONS[34]),
        "edgion_gateway_fips_host_detect_error" => Some(&DEFINITIONS[35]),
        "edgion_gateway_fips_policy_active" => Some(&DEFINITIONS[36]),
        "edgion_wasm_module_load_total" => Some(&DEFINITIONS[37]),
        "edgion_wasm_module_compile_duration_ms" => Some(&DEFINITIONS[38]),
        "edgion_wasm_module_released_total" => Some(&DEFINITIONS[39]),
        "edgion_wasm_instance_instantiate_total" => Some(&DEFINITIONS[40]),
        "edgion_wasm_instance_discard_total" => Some(&DEFINITIONS[41]),
        "edgion_wasm_pool_size" => Some(&DEFINITIONS[42]),
        "edgion_wasm_hook_duration_ms" => Some(&DEFINITIONS[43]),
        "edgion_wasm_trap_total" => Some(&DEFINITIONS[44]),
        "edgion_wasm_fail_mode_total" => Some(&DEFINITIONS[45]),
        "edgion_wasm_callout_total" => Some(&DEFINITIONS[46]),
        "edgion_wasm_modules_loaded" => Some(&DEFINITIONS[47]),
        "edgion_wasm_shared_data_bytes" => Some(&DEFINITIONS[48]),
        "edgion_wasm_body_pause_unsupported_total" => Some(&DEFINITIONS[49]),
        "edgion_gateway_plugin_policy_blocked_total" => Some(&DEFINITIONS[50]),
        "edgion_gateway_plugin_policy_bypassed_total" => Some(&DEFINITIONS[51]),
        "edgion_geoip_db_loaded" => Some(&DEFINITIONS[52]),
        "edgion_geoip_db_build_epoch_seconds" => Some(&DEFINITIONS[53]),
        "edgion_geoip_update_failed_total" => Some(&DEFINITIONS[54]),
        "edgion_geoip_lookup_panic_total" => Some(&DEFINITIONS[55]),
        "edgion_gateway_retry_budget_denied_total" => Some(&DEFINITIONS[56]),
        "edgion_conf_sync_cert_expiry_timestamp_seconds" => Some(&DEFINITIONS[57]),
        "edgion_system_error_state" => Some(&DEFINITIONS[58]),
        "edgion_fed_connections_active" => Some(&DEFINITIONS[59]),
        "edgion_fed_connection_events_total" => Some(&DEFINITIONS[60]),
        "edgion_fed_connection_duration_seconds_last" => Some(&DEFINITIONS[61]),
        "edgion_fed_watch_events_total" => Some(&DEFINITIONS[62]),
        "edgion_fed_ready_gate_wait_seconds_last" => Some(&DEFINITIONS[63]),
        "edgion_fed_rbac_denied_total" => Some(&DEFINITIONS[64]),
        "edgion_fed_kill_switch_state" => Some(&DEFINITIONS[65]),
        "edgion_fed_client_running" => Some(&DEFINITIONS[66]),
        "edgion_controller_resource_abnormal" => Some(&DEFINITIONS[67]),
        "edgion_controller_unknown_config_fields" => Some(&DEFINITIONS[68]),
        "edgion_controller_init_sync_duration_seconds_last" => Some(&DEFINITIONS[69]),
        "edgion_controller_active" => Some(&DEFINITIONS[70]),
        "edgion_controller_resources_watched" => Some(&DEFINITIONS[71]),
        "edgion_controller_leader" => Some(&DEFINITIONS[72]),
        "edgion_controller_leader_transitions_total" => Some(&DEFINITIONS[73]),
        "edgion_controller_reload_total" => Some(&DEFINITIONS[74]),
        "edgion_controller_reload_duration_seconds_last" => Some(&DEFINITIONS[75]),
        "edgion_controller_reload_client_notifications_total" => Some(&DEFINITIONS[76]),
        "edgion_controller_status_writes_total" => Some(&DEFINITIONS[77]),
        "edgion_controller_status_writes_skipped_total" => Some(&DEFINITIONS[78]),
        "edgion_controller_status_conditions_max" => Some(&DEFINITIONS[79]),
        "edgion_controller_unexpected_apply_during_init_total" => Some(&DEFINITIONS[80]),
        "edgion_controller_leader_job_alive" => Some(&DEFINITIONS[81]),
        "edgion_metrics_series_rejected_total" => Some(&DEFINITIONS[82]),
        "edgion_metrics_cardinality_limit_active" => Some(&DEFINITIONS[83]),
        "edgion_backend_requests_total" => Some(&DEFINITIONS[84]),
        "edgion_gateway_request_duration_ms" => Some(&DEFINITIONS[85]),
        "edgion_gateway_request_bytes_total" => Some(&DEFINITIONS[86]),
        "edgion_gateway_response_bytes_total" => Some(&DEFINITIONS[87]),
        "edgion_gateway_genai_tokens_total" => Some(&DEFINITIONS[88]),
        "edgion_mirror_requests_total" => Some(&DEFINITIONS[89]),
        "edgion_conn_ip_restriction_events_total" => Some(&DEFINITIONS[90]),
        "edgion_geoip_denied_total" => Some(&DEFINITIONS[91]),
        "edgion_connection_rate_limit_rejected_total" => Some(&DEFINITIONS[92]),
        "edgion_connection_rate_limit_redis_failure_total" => Some(&DEFINITIONS[93]),
        "edgion_circuit_breaker_ejections_total" => Some(&DEFINITIONS[94]),
        "edgion_circuit_breaker_ejected_endpoints" => Some(&DEFINITIONS[95]),
        "edgion_circuit_breaker_rejections_total" => Some(&DEFINITIONS[96]),
        "edgion_gateway_circuit_breaker_shed_total" => Some(&DEFINITIONS[97]),
        "edgion_lb_degrade_selections_total" => Some(&DEFINITIONS[98]),
        "edgion_rr_selector_truncated_total" => Some(&DEFINITIONS[99]),
        "edgion_region_route_overlay_events_total" => Some(&DEFINITIONS[100]),
        "edgion_acme_challenge_lookup_total" => Some(&DEFINITIONS[101]),
        "edgion_link_sys_redis_connections" => Some(&DEFINITIONS[102]),
        "edgion_link_sys_redis_topology_events_total" => Some(&DEFINITIONS[103]),
        _ => None,
    }
}

/// Validate once while preparing a typed descriptor; no user-supplied class flag exists.
pub fn validate(key: &Key, kind: MetricType, class: MetricClass) -> bool {
    let Some(def) = definition(key.name()) else {
        return false;
    };
    if def.kind != kind || def.class != class || class == MetricClass::Snapshot {
        return false;
    }
    let mut count = 0;
    let mut bytes = key.name().len();
    let mut present = 0u32;
    for label in key.labels() {
        count += 1;
        bytes = bytes
            .saturating_add(label.key().len())
            .saturating_add(label.value().len());
        if label.value().len() > MAX_LABEL_BYTES || count > MAX_LABELS || bytes > MAX_KEY_BYTES {
            return false;
        }
        let Some(index) = def.labels.iter().position(|allowed| allowed.key == label.key()) else {
            return false;
        };
        let bit = 1u32 << index;
        if present & bit != 0 {
            return false;
        }
        present |= bit;
        match def.labels[index].domain {
            LabelDomain::Closed(values) if !values.contains(&label.value()) => return false,
            LabelDomain::Kind
                if !matches!(label.value(), "unknown" | "*" | "RegionRoute" | "NonResource")
                    && !edgion_resources::resource::ALL_RESOURCE_INFOS
                        .iter()
                        .any(|info| info.kind.as_str() == label.value()) =>
            {
                return false
            }
            LabelDomain::Open if class == MetricClass::Fixed => return false,
            _ => {}
        }
    }
    def.labels
        .iter()
        .enumerate()
        .all(|(index, label)| !label.required || present & (1u32 << index) != 0)
}

#[cfg(test)]
#[path = "../../tests/unit/observe/metrics_catalog_test.rs"]
mod tests;
