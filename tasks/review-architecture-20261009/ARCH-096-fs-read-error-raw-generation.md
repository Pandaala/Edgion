---
id: ARCH-096
status: candidate
kind: behavior-defect
priority: P2
partition: PART-398
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-096: Remove the previous raw generation when the FS worker cannot read its source

## Problem and current ownership

The FileSystem worker revokes a processed resource after a file-read failure but leaves its previous object in LocalRawStore. The same worker removes raw state when parsing fails, source identity changes, or the source file disappears. The read-error branch therefore gives the raw and processed owners inconsistent meanings for an unavailable current generation.

FileSystemResourceController injects its LocalRawStore into ResourceProcessor. In resource_controller.rs:648-754, the worker reads the registry-selected winner, or the existing canonical fallback. A successful read and parse upserts the object actually processed. Parse failure removes the raw object and records ParseError; identity mismatch and missing-file handling also remove raw state. However, read_to_string returning Err at733-745 only builds an error message and returns (None, Some(error_msg)).

The common error branch at758-779 calls process_work_item with no source object and writes ParseError inline. ResourceProcessor::process_work_item maps a missing source with an existing cached object to process_delete. That runs the existing cleanup hooks, removes the processed cache entry and invokes after_unpublish. Neither this processor operation nor the shared RawStore interface removes the local raw entry; that mutation belongs to the FileSystem resource controller.

## Concrete supported witness and consequence

Start with a valid resource in both the raw store and processed cache. Its key is enqueued by an ordinary runtime Apply or an existing dependency requeue. Before the worker reads the winner file, an external file edit makes its bytes invalid UTF-8. The file still exists, so read_to_string fails rather than taking the missing-file branch. The worker revokes the processed generation and persists ParseError, yet LocalRawStore continues returning the previous valid object.

This witness does not require the watcher to deliver an invalid byte document, an unsupported pre-initialization Apply, a lost broadcast event, or an Admin API request that bypasses its UTF-8 check. It concerns the worker's supported reread of an already queued key. Another valid Apply or requeue can later repair the state, but this pass does not itself remove the stale raw generation.

RawStore has real consumers independent of ServerCache. ProcessorObj::get_raw_any reads the injected raw store directly. route_utils::lookup_gateway intentionally uses it to find unfiltered Gateway input, including foreign-class Gateways omitted from the processed cache. validate_parent_ref_accepted performs its parent-existence and source-listener lookup through that function. A Gateway affected by this read failure can consequently remain visible as its previous raw specification after processed publication has been withdrawn.

This is not a claim that the old listener publication necessarily remains active. Production listener attachment resolution principally uses serving_gateway, and the Gateway after_unpublish hook removes its published listener configuration and requeues consumers. The defect is the retained raw source generation and the resulting stale source lookup.

## Bounded improvement

Remove work_item.key from fs_raw_store in the existing read_to_string error branch before entering the existing no-source/error-status path. Reuse LocalRawStore::remove and preserve the current key, selected path, diagnostic reason, cache-revocation hooks, trigger chain, inline status ordering and workqueue completion.

Do not create a RuntimeInvalidEnvelope merely to share the parse-error shape. An IO failure may be transient: the current branch permits a later ordinary requeue to read a repaired file. An envelope would make the worker's early invalid-envelope guard skip that reread until another event clears it. Preserve the current nonsticky read-error recovery behavior.

Keep raw input distinct from processed publication. Replacing legitimate raw consumers with processed-cache reads would change foreign-Gateway and other unfiltered-source semantics. This fix adds no source cache, failure framework, retry timer, asynchronous status writer, metric or log. It also makes no cross-kind atomicity or generation-fencing guarantee beyond the existing serial worker model.

## Original decisions, alternatives and deduplication

The FileSystem architecture assigns raw-store maintenance to both the event loop and the worker so the object read by the worker corrects source-version drift. The original processed-cache review explicitly retains RawStore for unfiltered source lookups and requires choosing the appropriate lifecycle view. Neither decision accepts keeping the previous raw object after this worker has chosen to revoke its processed generation.

The supported invalid-input decision distinguishes runtime malformed generations from defensive pre-init Apply handling. This candidate concerns the initialized worker. The accepted inline status-IO and no-status-retry policies remain intact.

ARCH-086 concerns etcd ingress discarding invalid UTF-8 before delivering a lifecycle event. ARCH-093 concerns initial FS broadcast delivery. Here a work item reaches the FS worker, and its read-error outcome performs incomplete raw-state cleanup. ARCH-089's schema diagnostic redaction and ARCH-091's call-local parsing ownership are separate causes.

Retaining both raw and processed last-valid state would require a different read-failure policy, contrary to the existing revocation performed here. Adding a sticky invalid envelope or a retry subsystem is unnecessary for this bounded correction.

## Validation and limits

The complete current resource_controller_test.rs covers initial parse rejection, identity checks, duplicate initialization, worker deletion, malformed runtime generations, repaired sidecars and dequeue-time identity drift. It does not assert raw/cache consistency after read_to_string fails. The existing private FS worker harness already exposes the processor, raw store, registry, status handler and worker handle.

Add focused coverage using an existing cached/raw resource, an existing winner file containing invalid UTF-8, and the real worker queue. Assert that the raw and processed entries are both absent after the failure, ParseError is persisted, and no sticky invalid envelope prevents recovery after a valid file is restored and the key is requeued. If asserting get_raw_any, inject the harness raw store through the existing set_raw_store method as the production constructor does. Keep tests in the established source-relative unit-test module; no production visibility expansion or backend mock framework is needed.

These are static source conclusions and proposed acceptance checks. No tests, builds, runtime reproduction or filesystem fault injection were executed. No production incident, measured performance effect or bounded recovery time is claimed.
