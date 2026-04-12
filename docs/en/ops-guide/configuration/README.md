# Configuration Reference

Edgion uses a three-tier configuration system covering process-level and GatewayClass-level dimensions.

## Configuration Tiers

| Tier | Carrier | Scope | How to Apply |
|------|---------|-------|--------------|
| Controller process config | TOML file | Single Controller instance | Restart process |
| Gateway process config | TOML file | Single Gateway instance | Restart process |
| Runtime config | EdgionGatewayConfig CRD | Entire GatewayClass | Takes effect dynamically |

## Contents

| Document | Description |
|----------|-------------|
| [controller-config.md](controller-config.md) | Controller TOML complete configuration parameters |
| [gateway-config.md](gateway-config.md) | Gateway TOML complete configuration parameters |
| [edgion-gateway-config.md](edgion-gateway-config.md) | EdgionGatewayConfig CRD — GatewayClass-level runtime configuration |
