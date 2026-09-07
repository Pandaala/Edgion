---
knowledge_type: compatibility-constraints
---

# Response-body sink extras and transform order

`ProxyHttp::upstream_response_body_filter` runs after Pingora's
`Session::upstream_compression`. The current body and chunks queued through
`ResponseBodySink` therefore use the post-codec representation; the sink does
not re-run that codec. Cache range slicing also precedes delivery of sink
extras, so an emitting consumer must disable range eligibility at the header
phase rather than append unsliced bytes to a ranged response.

This is not reachable through Edgion's current emitter. A streaming processor
that may inject or terminate declares `changes_body_length()`, and Edgion
removes `Content-Length` at both response-header boundaries. That disables both
Edgion's known-length-only compression and Pingora's cache range selection.
Downstream `ProxyHttp::response_body_filter` and downstream HTTP modules do see
sink extras: all three pumps drain extras into the output task slice before
running those stages.

Do not re-report this as an Edgion response-corruption defect unless a current
emitter can retain range eligibility or activate `upstream_compression`. For a
new generic Pingora consumer, treat the ordering above as part of the hook's
representation contract.
