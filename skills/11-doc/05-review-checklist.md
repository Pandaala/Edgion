# Documentation Review Checklist

Use this checklist when reviewing documents under `docs/`. Ordered by priority: technical accuracy > completeness > consistency > formatting.

## Technical accuracy

- [ ] **Parameter table matches code**: parameter names, types, defaults match the actual schema/code
- [ ] **YAML examples work**: examples can be applied directly with `kubectl apply` (or with minor edits); apiVersion/kind/metadata are complete
- [ ] **Behavior described accurately**: feature behavior described in the doc matches actual code behavior
- [ ] **Version info correct**: minimum version requirements and Breaking Change version numbers are accurate
- [ ] **No stale content**: no removed fields, deprecated usage, or changed defaults

## Completeness

- [ ] **All required sections present**: check required sections per the page template ([02-page-template.md](02-page-template.md))
- [ ] **Parameter table complete**: every user-configurable parameter is listed
- [ ] **Common scenarios covered**: at least 3 scenario examples (plugin pages)
- [ ] **Complete configuration example**: at least one production-grade complete YAML (including the Route envelope)
- [ ] **Related-documents links**: there is a related-documents section at the end of the page

## Multilingual sync

- [ ] **zh-CN and en are symmetric**: both language versions have the file
- [ ] **Section structure consistent**: same number, order, and level of sections
- [ ] **YAML examples identical**: code-block content is identical; only comment language differs
- [ ] **Parameter tables match**: parameter names, types, and defaults are identical; only the description column differs by language
- [ ] **No translation gaps**: no paragraphs that mix Chinese and English (except for terms that are deliberately not translated)

## Link validity

- [ ] **Internal links valid**: all relative-path internal links point to existing files
- [ ] **Anchors valid**: links with `#section` anchors point to anchors that exist in the target file
- [ ] **External links reasonable**: external links point to authoritative sources (MDN, K8s official, RFC), not blogs or pages likely to break
- [ ] **No links to skills/**: documents under docs/ do not reference the skills/ directory directly

## Writing style

- [ ] **Person correct**: address the reader as "you", not "users"
- [ ] **Concise headings**: H1 is the feature name, with no redundant prefix
- [ ] **Edgion-extension callout**: non-standard Gateway API features have the `🔌 Edgion extension` callout
- [ ] **No internal implementation details**: no handler flows, source-code locations, or Rust data structures (dev-guide is the exception)
- [ ] **Terminology consistent**: terms follow the glossary; standard Gateway API terms are not translated

## Formatting and typography

- [ ] **Parameter table format**: four columns (parameter / type / default / description); types use the unified labels
- [ ] **Code-block tags**: YAML uses `yaml`, Shell uses `bash`, logs use `text`
- [ ] **Right / wrong example markers**: ✅/❌ to indicate correct vs. incorrect approaches
- [ ] **Chinese typography**: spaces between Chinese and English; full-width punctuation
- [ ] **Security warnings**: development-only examples involving security carry ⚠️ markers

## Structure and navigation

- [ ] **Section order correct**: follows progressive complexity (overview → configuration → scenarios → advanced → FAQ → links)
- [ ] **README.md updated**: for new documents, the README.md of the containing directory is updated
- [ ] **Site navigation updated**: when adding / removing / moving user-facing documents, VitePress sidebar/nav entries are in sync

## Security review

- [ ] **No sensitive information**: examples contain no real tokens, passwords, certificate contents, or internal IPs
- [ ] **Security warnings in place**: insecure configurations (such as wildcard `*`) carry explicit security warnings
- [ ] **Production vs. development**: clearly state which configurations are for development environments only

## Quick review mode

If time is limited, check at least these 5 items:

1. ✅ YAML examples can be applied with `kubectl apply`
2. ✅ Parameter tables match the code
3. ✅ Both zh-CN and en have the corresponding files
4. ✅ No sensitive information
5. ✅ Internal links are valid
