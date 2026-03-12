#!/usr/bin/env bash
#
# generate_k8s_conf.sh -  K8s 
#
# ：
#   1.  Endpoint/EndpointSlice （K8s ）
#   2.  Service  selector  test-server Pod
#   3.  namespace  deployment 
#   4. 
#
# ：
#   ./generate_k8s_conf.sh []
#
# ：
#   ./generate_k8s_conf.sh              #  examples/k8stest/conf
#   ./generate_k8s_conf.sh /tmp/k8s     #  /tmp/k8s
#

set -euo pipefail

# 
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# test 
TEST_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# k8stest 
K8S_TEST_DIR="$(cd "$TEST_DIR/.." && pwd)/k8stest"
# conf 
CONF_DIR="$TEST_DIR/conf"
# 
OUTPUT_DIR="${1:-$K8S_TEST_DIR/conf}"
# Deployment （ workspace root）
WORKSPACE_ROOT="$(cd "$TEST_DIR/../../.." && pwd)"
DEPLOYMENT_SRC="$WORKSPACE_ROOT/edgion-deploy/kubernetes/test/test-server/deployment.yaml"

# 
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
    exit 1
}

#  yq 
check_yq() {
    if command -v yq &> /dev/null; then
        echo "yq"
    else
        echo "sed"
    fi
}

#  Endpoint/EndpointSlice
is_endpoint_resource() {
    local file="$1"
    local filename=$(basename "$file")
    
    #  Endpoint/EndpointSlice 
    if [[ "$filename" == EndpointSlice* || "$filename" == Endpoint* ]]; then
        return 0
    fi
    
    #  kind （， kind）
    if grep -qE "^kind:[[:space:]]*Endpoint(Slice)?[[:space:]]*$" "$file" 2>/dev/null; then
        return 0
    fi
    
    return 1
}

#  Service
is_service() {
    local file="$1"
    local filename=$(basename "$file")
    
    #  Service （ Service_xxx.yaml）
    if [[ "$filename" == Service* ]]; then
        return 0
    fi
    
    #  kind （， targetRefs  kind: Service）
    if grep -qE "^kind:[[:space:]]*Service" "$file" 2>/dev/null; then
        return 0
    fi
    
    return 1
}

#  yq  Service  selector
add_selector_yq() {
    local input="$1"
    local output="$2"
    
    yq eval '.spec.selector = {"app": "edgion-test-server"}' "$input" > "$output"
}

#  sed  Service  selector（fallback）
add_selector_sed() {
    local input="$1"
    local output="$2"
    
    #  selector
    if grep -q "selector:" "$input"; then
        #  selector，
        sed 's/selector:.*/selector:\n    app: edgion-test-server/' "$input" > "$output"
    else
        #  selector， spec: 
        awk '
        /^spec:/ {
            print
            found_spec = 1
            next
        }
        found_spec && /^  [a-z]/ && !added_selector {
            print "  selector:"
            print "    app: edgion-test-server"
            added_selector = 1
        }
        { print }
        END {
            if (found_spec && !added_selector) {
                print "  selector:"
                print "    app: edgion-test-server"
            }
        }
        ' "$input" > "$output"
    fi
}

#  Service 
process_service() {
    local input="$1"
    local output="$2"
    local tool="$3"
    
    if [[ "$tool" == "yq" ]]; then
        add_selector_yq "$input" "$output"
    else
        add_selector_sed "$input" "$output"
    fi
}

#  namespace 
generate_namespace() {
    local output="$1"
    
    cat > "$output" << 'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: edgion-test
  labels:
    app.kubernetes.io/name: edgion-test
    app.kubernetes.io/component: test
EOF
}

#  Deployment（ namespace）
process_deployment() {
    local input="$1"
    local output="$2"
    local tool="$3"
    
    if [[ "$tool" == "yq" ]]; then
        yq eval '.metadata.namespace = "edgion-test"' "$input" > "$output"
    else
        sed 's/namespace: edgion-system/namespace: edgion-test/g' "$input" > "$output"
    fi
}

# 
main() {
    info " K8s "
    info ": $CONF_DIR"
    info ": $OUTPUT_DIR"
    
    # 
    if [[ ! -d "$CONF_DIR" ]]; then
        error ": $CONF_DIR"
    fi
    
    #  deployment 
    if [[ ! -f "$DEPLOYMENT_SRC" ]]; then
        error "Deployment : $DEPLOYMENT_SRC"
    fi
    
    # 
    local tool=$(check_yq)
    info " $tool  YAML"
    
    # 
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    
    # 
    local total=0
    local skipped_endpoint_like=0
    local services=0
    local copied=0
    
    # 1.  namespace
    generate_namespace "$OUTPUT_DIR/00-namespace.yaml"
    info " 00-namespace.yaml"
    
    # 2.  deployment
    process_deployment "$DEPLOYMENT_SRC" "$OUTPUT_DIR/01-deployment.yaml" "$tool"
    info " 01-deployment.yaml (namespace: edgion-test)"
    
    # 3.  YAML 
    while IFS= read -r -d '' file; do
        ((total++))
        
        # 
        local rel_path="${file#$CONF_DIR/}"
        local output_file="$OUTPUT_DIR/$rel_path"
        local output_dir=$(dirname "$output_file")
        
        #  Endpoint/EndpointSlice
        if is_endpoint_resource "$file"; then
            ((skipped_endpoint_like++))
            warn " Endpoint/EndpointSlice: $rel_path"
            continue
        fi
        
        # 
        mkdir -p "$output_dir"
        
        #  Service
        if is_service "$file"; then
            ((services++))
            process_service "$file" "$output_file" "$tool"
            info " Service: $rel_path ( selector)"
        else
            # 
            ((copied++))
            cp "$file" "$output_file"
        fi
        
    done < <(find "$CONF_DIR" -name "*.yaml" -type f -print0)
    
    echo ""
    info "==========  =========="
    info ": $total"
    info " Endpoint/EndpointSlice: $skipped_endpoint_like"
    info " Service: $services"
    info ": $copied"
    info ": $OUTPUT_DIR"

    # 4. ： Endpoint/EndpointSlice
    local endpoint_kinds
    endpoint_kinds="$(rg -n "^[[:space:]]*kind:[[:space:]]*Endpoint(Slice)?[[:space:]]*$" "$OUTPUT_DIR" -S || true)"
    if [[ -n "$endpoint_kinds" ]]; then
        error " Endpoint/EndpointSlice kind:\n$endpoint_kinds"
    fi

    local endpoint_files
    endpoint_files="$(find "$OUTPUT_DIR" -type f \( -name '*.yaml' -o -name '*.yml' \) | grep -E '/[^/]*Endpoint(Slice)?[^/]*\.ya?ml$' || true)"
    if [[ -n "$endpoint_files" ]]; then
        error " Endpoint/EndpointSlice-like :\n$endpoint_files"
    fi

    info "： Endpoint/EndpointSlice"
    echo ""
    info ":"
    echo "  kubectl apply -f $OUTPUT_DIR/00-namespace.yaml"
    echo "  kubectl apply -f $OUTPUT_DIR/01-deployment.yaml"
    echo "  kubectl apply -Rf $OUTPUT_DIR/<>"
    echo ""
    info ":"
    echo "  kubectl apply -Rf $OUTPUT_DIR"
}

main "$@"
