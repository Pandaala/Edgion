# Align the LinkSys source readme with prepared HTTPDNS ownership

The HttpDns row in edgion-gateway/src/link_sys/readme claims raw configuration
lookup without a typed client. Current runtime/store.rs publishes
ProviderRuntime::HttpDns(Arc<PreparedHttpDns>) and exposes get_httpdns_client;
backend and AI consumers use the prepared core. The original current decision is
skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md.

A later documentation change should update that row to describe the current
prepared handle. No source documentation is changed by this read-only review.
This is a documentation discrepancy, not a claim of a runtime or new cache defect.
