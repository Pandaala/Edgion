"""Silent production-cache barrier before restarting with the first listener set."""
import json
from pathlib import Path
import sys
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


RESPONSE_CAP = 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def fixture_present(kind, payload):
    """Match only the owned fixture in the real ListResponse resource envelope."""
    if not isinstance(payload, dict) or payload.get("success") is not True:
        return False
    resources = payload.get("data")
    if not isinstance(resources, list):
        return False
    for resource in resources:
        if not isinstance(resource, dict):
            continue
        metadata, spec = resource.get("metadata"), resource.get("spec")
        if not isinstance(metadata, dict) or not isinstance(spec, dict):
            continue
        if metadata.get("name") != "package-test":
            continue
        if kind == "GatewayClass":
            if spec.get("controllerName") == "edgion.io/gateway-controller":
                return True
        elif kind == "Gateway":
            if metadata.get("namespace") != "default" or spec.get("gatewayClassName") != "package-test":
                continue
            listeners = spec.get("listeners")
            if not isinstance(listeners, list):
                continue
            for listener in listeners:
                if (isinstance(listener, dict) and listener.get("protocol") == "HTTP"
                        and type(listener.get("port")) is int and listener["port"] == 80):
                    return True
    return False


def probe(kind, token):
    # Keep credentials and resources private, even when the server fails.
    request = Request("http://127.0.0.1:12001/configclient/" + kind + "/list",
                      headers={"Authorization": "Bearer " + token})
    opener = build_opener(ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=2) as response:
        if response.status != 200:
            return False
        body = response.read(RESPONSE_CAP + 1)
    if len(body) > RESPONSE_CAP:
        return False
    return fixture_present(kind, json.loads(body))


def main(state):
    try:
        token = Path(state, "gateway-token").read_text()
        return 0 if probe("GatewayClass", token) and probe("Gateway", token) else 1
    except Exception:
        # No exception, body, or token is suitable for the lifecycle report.
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
