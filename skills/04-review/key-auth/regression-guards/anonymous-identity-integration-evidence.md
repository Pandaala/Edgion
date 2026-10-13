---
name: anonymous-identity-integration-evidence
description: Require distinct anonymous and authenticated identities in KeyAuth integration echoes.
knowledge_type: regression-guards
---

# KeyAuth anonymous identity evidence (ARCH-377)

The two anonymous-enabled KeyAuth cases must inspect the completed `/headers`
JSON envelope. HTTP 200 alone cannot distinguish anonymous fallback from a valid
credential. The no-key case requires `x-consumer-username: anonymous-user` and
`x-anonymous-consumer: true`. The valid-key case requires username `jack` and no
affirmative anonymous marker. Missing, malformed or unreadable observations fail.
Diagnostics name the failed observation without printing echoed headers or keys.

Both cases keep their original requests, fixture references, listener selection,
deadlines and registration. No client identity headers manufacture the evidence.
Pure validator tests cover valid branches, missing or wrong identities, invalid
envelopes, non-string fields and safe failure text. They do not establish a fresh
live local or Kubernetes run.

Re-evaluate when KeyAuth identity publication, anonymous fallback, fixture key
metadata or the backend `/headers` envelope changes.
