---
name: ctl-architecture
description: edgion-cli CLI tool architecture — three target modes, subcommands, output formats, HTTP client.
---

# 04 Ctl architecture (edgion-cli)

> edgion-cli is Edgion's command-line management tool, interacting with Controller/Gateway via HTTP API.
> Supports three target modes; can compare the three-tier center/server/client cache for sync troubleshooting.
> Here `center` means the Controller's ConfCenter CRUD API; it is unrelated to the federation Center in the separate EdgionCenter repository.

## File index

| File | Topic | Recommended reading scenario |
|------|------|-------------|
| [00-overview.md](00-overview.md) | Overview + target modes + API routes | Understanding ctl design and debugging methods |
| [01-commands.md](01-commands.md) | Subcommand details | Using/extending ctl commands |

## Quick reference

```bash
# View all HTTPRoutes in center (ConfCenter API)
edgion-cli get httproute

# View HTTPRoutes in server (Controller cache)
edgion-cli -t server get httproute

# View HTTPRoutes in client (Gateway cache)
edgion-cli -t client get httproute

# Compare center and client to troubleshoot sync issues
edgion-cli get httproute -o json > center.json
edgion-cli -t client get httproute -o json > client.json
diff center.json client.json

# Apply configuration
edgion-cli apply -f config.yaml
edgion-cli apply -f config-dir/


# Generate conf_sync gRPC mTLS certificates
edgion-cli gen-certs --output file --output-dir certs/grpc/

# Generate and persist bearer-token material
edgion-cli gen-token
edgion-cli login --token <token>
```
