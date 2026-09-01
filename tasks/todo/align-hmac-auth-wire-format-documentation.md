# Align HmacAuth Documentation with the Runtime Wire Format

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | documentation-correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0081 |

## AI guide summary

Make the English and Chinese HmacAuth user guides describe the authentication wire format that the runtime actually accepts.

## Requirements

- Update `docs/en/user-guide/http-route/filters/edgion-plugins/hmac-auth.md` and its Chinese counterpart together.
- Describe `Authorization` and `Proxy-Authorization` as the accepted credential sources, with the `hmac` scheme and `username` parameter.
- Do not claim that `Signature` is accepted for authentication. Explain narrowly that `hideCredentials` removes both accepted authorization headers plus `Signature` before proxying.
- Add the missing YAML list marker to every `requestPlugins` HmacAuth entry.
- Replace or clearly label bare `filters` fragments with the real EdgionPlugins `requestPlugins` carrier.
- Use a valid request date in signed examples and keep canonical request-target, header ordering, algorithm, and signature descriptions consistent with runtime behavior.
- Do not add legacy `Signature keyId` parsing, aliases, compatibility shims, or integration-test edits.

## Acceptance criteria

- Every English and Chinese request example is syntactically valid and matches the runtime parser.
- Resource examples apply to the real EdgionPlugins schema shape.
- Credential hiding is documented without implying an additional authentication source.
- Documentation validation and ordinary repository checks pass.
