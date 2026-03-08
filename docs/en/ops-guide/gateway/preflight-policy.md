# Preflight Request Handling Policy

> **🔌 Edgion Extension**
> 
> The preflight policy is configured through the `EdgionGatewayConfig` CRD and is an Edgion extension feature.

## What is Preflight?

Preflight is an OPTIONS request automatically sent by browsers before making cross-origin requests, used to check if the server allows the actual request.

**Example**:
- The browser wants to send a `POST` request to `https://api.example.com`
- The browser first automatically sends an `OPTIONS` request to check permissions
- The server returns allowed methods and header information
- The browser then sends the actual `POST` request

## Default Behavior

Edgion Gateway **automatically handles all preflight requests** without additional configuration.

- If the route has a CORS plugin configured, it responds using the CORS configuration
- If there is no CORS plugin, it returns `204 No Content`

## Custom Configuration (Optional)

If you need to customize preflight handling behavior, configure it in `EdgionGatewayConfig`:

```yaml
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: edgion-gateway-config
spec:
  preflightPolicy:
    mode: cors-standard        # or all-options
    statusCode: 204            # default status code
```

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | String | `cors-standard` | Preflight detection mode, see below |
| `statusCode` | Integer | `204` | Default response status code when CORS is not configured |

### mode Options

#### `cors-standard` (Recommended)

Strict detection conforming to the CORS standard:

- Request method must be `OPTIONS`
- Must include the `Origin` header
- Must include the `Access-Control-Request-Method` header

**Use case**: Standard browser cross-origin requests

#### `all-options`

Treats all `OPTIONS` requests as preflight:

- Handles any request with the `OPTIONS` method
- Does not check other headers

**Use cases**:
- Certain non-standard clients
- Need to uniformly handle all OPTIONS requests

## Usage Examples

### Scenario 1: Using Default Configuration (No Configuration Needed)

In most cases, the default configuration is sufficient, requiring no additional setup.

### Scenario 2: Custom Preflight Detection Mode

```yaml
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: edgion-gateway-config
spec:
  preflightPolicy:
    mode: all-options
```

### Scenario 3: Custom Default Status Code

```yaml
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: edgion-gateway-config
spec:
  preflightPolicy:
    statusCode: 200
```

## Relationship with CORS Plugin

The preflight policy works **in conjunction with** the CORS plugin:

1. **Preflight Handler executes first**: Intercepts preflight requests before all plugins
2. **Automatically looks up CORS configuration**: Searches for CORS plugin configuration in the route's plugin list
3. **Uses CORS response**: If CORS configuration is found, responds according to CORS rules
4. **Default response**: If no CORS configuration is found, returns the configured default status code

**Example flow**:

```
Browser OPTIONS request
    |
Preflight Handler intercepts
    |
Check if route has CORS plugin configured?
    |-- Yes -> Respond using CORS configuration
    +-- No  -> Return 204 No Content
```

## FAQ

### Q: Do I need to configure preflight?

**A:** In most cases, no. The default configuration already correctly handles standard CORS preflight requests.

### Q: When should I use `all-options` mode?

**A:** When you encounter the following situations:
- Certain clients send OPTIONS requests without standard CORS headers
- Need to uniformly handle all OPTIONS requests, regardless of whether they are CORS preflight

### Q: Will preflight responses go through other plugins?

**A:** No. Preflight requests are intercepted and responded to **before** all plugins (including authentication, rate limiting, etc.), which is standards-compliant behavior.

### Q: How to view preflight request processing logs?

**A:** Preflight requests are recorded in `access.log` and can be filtered by the HTTP method `OPTIONS`:

```bash
grep "OPTIONS" logs/access.log
```

## Related Documentation

- [CORS Plugin Configuration](../../user-guide/http-route/filters/edgion-plugins/cors.md)
- [Resource Architecture Overview (Dev Guide)](../../dev-guide/resource-architecture-overview.md)
