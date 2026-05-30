# TLSRoute

TLSRoute routes encrypted TLS connections to backends based on the SNI (Server Name Indication) in the TLS ClientHello, without terminating TLS at the gateway (TLS passthrough).

## Contents

| Document | Description |
|----------|-------------|
| overview.md | TLSRoute concepts, structure, and a complete SNI-passthrough example |
| backends/ | (directory) How to define and weight the backends a TLSRoute forwards to |
