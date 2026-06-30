# Wire EdgionStatus::OutboundTlsSanMismatch into access log

When `OutboundSanVerifier` rejects a peer cert at outbound handshake time,
the rustls error eventually surfaces in the plugin's outbound auth/connect
failure path, but is reported as a generic auth failure — the new
`EdgionStatus::OutboundTlsSanMismatch (999_09)` variant is currently
unused.

## To do

1. ldap_auth: in `ldap_bind` error handling, detect `LdapError` carrying
   "SAN mismatch:" substring → emit OutboundTlsSanMismatch
2. oidc: in `discovery_jwks` reqwest error handling, same
3. Add per-request access-log channel for outbound TLS failures
   (gateway today only has ssl_log for inbound TLS)
4. Update fixture READMEs to assert EdgionStatus: 999_09
