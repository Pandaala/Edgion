# waf3-03: XML text values are split at every comment, CDATA and PI boundary, so `SEL<!---->ECT` style payload splitting evades `XML:/*` rules

- **Status**: implemented in the T14 D-3 decision commit (`feat(waf)!: concatenate XML character data across comments, PIs and CDATA`, deep element content), awaiting user keep/revert
- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/xml_stream.rs:1106-1110` (`State::Text` on `<`: `emit_token(true, ...)` publishes the text collected so far, then starts markup)
  - `edgion-waf/src/seclang/xml_stream.rs:1292-1303` (comments are consumed without emitting and return to `State::Text`, starting a new token)
  - `edgion-waf/src/seclang/xml_stream.rs:1304-1323`, `538-571` (CDATA is emitted as its own separate value)
  - `edgion-waf/src/seclang/xml_stream.rs:574-637` (fast path also terminates a text value at `<`)
  - `edgion-waf/src/seclang/json_inspection_collector.rs` `XmlStreamSink::value` (each emitted fragment becomes an independent `XML` candidate)

## Summary
The native XML projection publishes every run of character data between two markup tokens as a separate
`XML` value. Comments, processing instructions and CDATA sections therefore cut one logical text node
into several independently inspected fragments. A request such as
`<id>1 UNION SEL<!---->ECT password FROM users</id>` or
`<q>&lt;scr<![CDATA[ipt>alert(1)</scr]]>ipt&gt;</q>` presents CRS with `"1 UNION SEL"` and
`"ECT password FROM users"`, neither of which matches, while DOM/binding parsers on the backend
(`getTextContent()`, `InnerText`, JAXB/XmlSerializer, `itertext()`) concatenate them into the attack string.

## Current behavior (evidence)
- In `advance` (`State::Text`), byte `<` calls `self.emit_token(true, None, sink)` and `begin_markup()`;
  after `-->` the state returns to `State::Text` with an empty token, so the following characters form a
  new value. CDATA content is emitted by `emit_token(false, ...)` (or the probe path at 538-571) as an
  additional value.
- The review guard `skills/04-review/waf/regression-guards/native-xml-inspection.md` and the user doc
  (`docs/en/user-guide/http-route/filters/edgion-plugins/waf.md`, "Normal text is emitted separately
  between markup") describe the projection but do not analyse splitting evasion.

## Industry comparison
- ModSecurity v2/v3 evaluate `XML:/*` with libxml2 XPath and use `xmlNodeGetContent` of the selected
  node: the root element's string value, i.e. all descendant text and CDATA concatenated, comments
  excluded. Splitting a keyword with a comment does not split the inspected value.
- The XPath data model's string-value of an element is the same concatenation; that is what CRS
  `XML:/*` rules were written and tested against.

## Impact
SQLi/XSS/RCE payloads in SOAP/XML APIs can be fragmented with zero-cost markup and bypass every
`XML:/*` rule (CRS 932/933/941/942 families list `XML:/*` as a target). The parser otherwise fails closed
on malformed XML, so this is the main structural differential left in the XML path.

## Suggested direction
Within one element, accumulate adjacent text, entity-decoded text and CDATA into one value and do not
flush at comments or PIs; flush only at a child start/end tag (bounded by the existing token/decoded
limits). Optionally also publish the element's concatenated descendant text (string-value) once per
element at close, bounded by `max_decoded_bytes`, to match ModSecurity's `XML:/*` more closely. Add
regression fixtures for comment-, PI- and CDATA-split keywords.

## Notes
Not covered by `waf-01..06` or other wave findings. The bounded streaming design is compatible with the
fix: merging fragments requires no DOM, only deferring the flush from `<!`/`<?`/`<![CDATA[` to the next
element tag.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `edgion-waf/src/seclang/xml_stream.rs:1106-1108`: in `State::Text`, a `<` calls `emit_token(true, ...)` and publishes the accumulated text as its own value. `emit_token` (`:705-725`) clears the token afterwards.
- Comment end (`:1292-1300`) returns to `State::Text` with an empty token. CDATA end (`:1305-1307`) emits CDATA as a separate value. So `SEL<!---->ECT` yields two candidates.
- `native-xml-inspection.md:41-44` and `waf.md:596` document separate emission as projection behavior but contain no evasion analysis. The comparison with ModSecurity `XML:/*` (libxml2 node content concatenation) is accurate.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-waf/src/seclang/xml_stream.rs` is unchanged; `State::Text` on `<` still calls `emit_token(true, ...)` (`:1106-1110`) and CDATA/comment handling (`:1292-1323`) still emits separate values.
- `native-xml-inspection.md` and the WAF user guide are unchanged (no evasion analysis added).
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
