# Add Malformed ClientHello Passthrough Coverage

## Meta

- Status: pending
- Type: test
- Priority: P3
- Source: Gateway campaign B0210 / D0118

## Scope and Acceptance

After the parser boundary fix is reviewed and landed, add explicitly authorized
wire coverage for malformed complete single-record ClientHello inputs. This task
is a test gap, not a claim of authentication bypass or successful malformed TLS.

- Independently corrupt handshake body, extension vector, SNI extension,
  ServerNameList and name-entry lengths while retaining a complete outer record.
- Verify the existing rejection path closes without forwarding bytes to a
  selected backend; distinguish parser rejection from later TLS handshake failure.
- Retain valid TLS 1.2/1.3 and ASCII-case cases, unknown extensions, partial socket
  reads and initial trailing-byte preservation.
- Keep the inspection cap and absolute deadline unchanged. Do not implement
  fragmented ClientHello reassembly or new hostname/duplicate-extension policy.
- Integrate only in a later authorized testing task; existing integration files
  remain immutable and no new integration test is added by this campaign.
