# Add Session Cookie Field Integration Coverage

## Status

Pending; discovered in B0195.

## Required Coverage

- For HTTP and gRPC, establish an affinity token and send it in a second Cookie
  field after an unrelated first field. Verify the issuing backend remains
  selected and the token is not refreshed.
- Send malformed expiry suffixes and verify a new session token is issued,
  rather than treating the malformed value as a valid bare hash.
- Reuse the existing shared integration topology and bounded synchronization.

Existing integration files remain frozen during the campaign. Current unit
coverage belongs beside the session-persistence implementation.
