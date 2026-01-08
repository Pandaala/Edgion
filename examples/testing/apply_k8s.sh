#!/bin/bash
#
# Edgion K8s Resource Apply Script
# This script applies Edgion resources to a Kubernetes cluster
#
# Usage: ./apply_k8s.sh [options]
# Options:
#   -n, --dry-run    Show what would be applied without actually applying
#   -d, --delete     Delete resources instead of applying
#   -h, --help       Show this help message
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_DIR="$SCRIPT_DIR/../conf"
CRD_DIR="$SCRIPT_DIR/../../config/crd"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Parse arguments
DRY_RUN=""
DELETE_MODE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -n|--dry-run)
            DRY_RUN="--dry-run=client"
            shift
            ;;
        -d|--delete)
            DELETE_MODE=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  -n, --dry-run    Show what would be applied without actually applying"
            echo "  -d, --delete     Delete resources instead of applying"
            echo "  -h, --help       Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check if kubectl is available
if ! command -v kubectl &> /dev/null; then
    echo -e "${RED}[ERROR]${NC} kubectl is not installed or not in PATH"
    exit 1
fi

# Check cluster connection
if ! kubectl cluster-info &> /dev/null; then
    echo -e "${RED}[ERROR]${NC} Cannot connect to Kubernetes cluster"
    exit 1
fi

echo -e "${BLUE}==========================================${NC}"
echo -e "${BLUE}    Edgion K8s Resource Apply Script${NC}"
echo -e "${BLUE}==========================================${NC}"
echo ""

if [ -n "$DRY_RUN" ]; then
    echo -e "${YELLOW}[DRY-RUN MODE]${NC} No changes will be made"
    echo ""
fi

if [ "$DELETE_MODE" = true ]; then
    echo -e "${YELLOW}[DELETE MODE]${NC} Resources will be deleted"
    echo ""
fi

# Function to apply/delete resources
apply_resource() {
    local file=$1
    local action="apply --server-side --force-conflicts"
    
    if [ "$DELETE_MODE" = true ]; then
        action="delete --ignore-not-found"
    fi
    
    echo -e "${GREEN}[+]${NC} $(basename "$file")"
    # Strip resourceVersion from the file before applying to avoid optimistic locking conflicts
    # Handle JSON format: "resourceVersion":"xxx", and ,"resourceVersion":"xxx"
    # Handle YAML format: resourceVersion: "xxx" or resourceVersion: xxx
    cat "$file" | sed -e 's/"resourceVersion":"[^"]*",//g' \
                      -e 's/,"resourceVersion":"[^"]*"//g' \
                      -e '/^[[:space:]]*resourceVersion:/d' \
        | kubectl $action -f - $DRY_RUN 2>&1 | sed 's/^/    /'
}

# Function to apply/delete resources from a pattern
apply_pattern() {
    local pattern=$1
    local dir=$2
    
    for file in "$dir"/$pattern; do
        if [ -f "$file" ]; then
            apply_resource "$file"
        fi
    done
}

# ==========================================
# Step 1: Create Namespaces
# ==========================================
echo -e "\n${BLUE}[Step 1] Creating Namespaces${NC}"
echo "-------------------------------------------"

if [ "$DELETE_MODE" = true ]; then
    echo -e "${YELLOW}[!]${NC} Skipping namespace deletion (manual cleanup required)"
else
    kubectl create namespace edgion-default $DRY_RUN --dry-run=client -o yaml | kubectl apply $DRY_RUN -f - 2>&1 | sed 's/^/    /'
    kubectl label namespace edgion-default app.kubernetes.io/part-of=edgion --overwrite $DRY_RUN 2>&1 | sed 's/^/    /' || true
    
    kubectl create namespace edgion-test $DRY_RUN --dry-run=client -o yaml | kubectl apply $DRY_RUN -f - 2>&1 | sed 's/^/    /'
    kubectl label namespace edgion-test app.kubernetes.io/part-of=edgion --overwrite $DRY_RUN 2>&1 | sed 's/^/    /' || true
fi

# ==========================================
# Step 2: Apply CRDs
# ==========================================
echo -e "\n${BLUE}[Step 2] Applying Custom Resource Definitions${NC}"
echo "-------------------------------------------"

# Gateway API CRDs (if not already installed)
if [ -d "$CRD_DIR/gateway-api" ]; then
    echo -e "${GREEN}[+]${NC} Applying Gateway API CRDs..."
    for file in "$CRD_DIR/gateway-api"/*.yaml; do
        if [ -f "$file" ]; then
            apply_resource "$file"
        fi
    done
fi

# Edgion CRDs
if [ -d "$CRD_DIR/edgion-crd" ]; then
    echo -e "${GREEN}[+]${NC} Applying Edgion CRDs..."
    for file in "$CRD_DIR/edgion-crd"/*.yaml; do
        if [ -f "$file" ]; then
            apply_resource "$file"
        fi
    done
fi

# ==========================================
# Step 3: Apply Core Resources (order matters)
# ==========================================
echo -e "\n${BLUE}[Step 3] Applying Core Resources${NC}"
echo "-------------------------------------------"

# 3.1 Secrets (needed by TLS configs)
echo -e "\n${GREEN}>>> Secrets${NC}"
apply_pattern "Secret_*.yaml" "$CONF_DIR"

# 3.2 Services (needed by routes)
echo -e "\n${GREEN}>>> Services${NC}"
apply_pattern "Service_*.yaml" "$CONF_DIR"

# 3.3 GatewayClass
echo -e "\n${GREEN}>>> GatewayClass${NC}"
apply_pattern "GatewayClass*.yaml" "$CONF_DIR"

# 3.4 LinkSys (external system connections)
echo -e "\n${GREEN}>>> LinkSys${NC}"
apply_pattern "LinkSys_*.yaml" "$CONF_DIR"

# 3.5 PluginMetaData
echo -e "\n${GREEN}>>> PluginMetaData${NC}"
apply_pattern "PluginMetaData_*.yaml" "$CONF_DIR"

# ==========================================
# Step 4: Apply Edgion Configurations
# ==========================================
echo -e "\n${BLUE}[Step 4] Applying Edgion Configurations${NC}"
echo "-------------------------------------------"

# 4.1 EdgionGatewayConfig
echo -e "\n${GREEN}>>> EdgionGatewayConfig${NC}"
apply_pattern "EdgionGatewayConfig_*.yaml" "$CONF_DIR"

# 4.2 EdgionTls
echo -e "\n${GREEN}>>> EdgionTls${NC}"
apply_pattern "EdgionTls_*.yaml" "$CONF_DIR"

# 4.3 EdgionPlugins
echo -e "\n${GREEN}>>> EdgionPlugins${NC}"
apply_pattern "EdgionPlugins_*.yaml" "$CONF_DIR"

# 4.4 EdgionStreamPlugins
echo -e "\n${GREEN}>>> EdgionStreamPlugins${NC}"
apply_pattern "EdgionStreamPlugins_*.yaml" "$CONF_DIR"

# 4.5 DebugAccessLogToHeader (custom plugin)
echo -e "\n${GREEN}>>> DebugAccessLogToHeader${NC}"
apply_pattern "DebugAccessLogToHeader_*.yaml" "$CONF_DIR"

# ==========================================
# Step 5: Apply Gateway API Resources
# ==========================================
echo -e "\n${BLUE}[Step 5] Applying Gateway API Resources${NC}"
echo "-------------------------------------------"

# 5.1 ReferenceGrant (needed for cross-namespace references)
echo -e "\n${GREEN}>>> ReferenceGrant${NC}"
apply_pattern "ReferenceGrant*.yaml" "$CONF_DIR"

# 5.2 BackendTLSPolicy
echo -e "\n${GREEN}>>> BackendTLSPolicy${NC}"
apply_pattern "BackendTLSPolicy_*.yaml" "$CONF_DIR"

# 5.3 Gateway
echo -e "\n${GREEN}>>> Gateway${NC}"
apply_pattern "Gateway_*.yaml" "$CONF_DIR"

# ==========================================
# Step 6: Apply Routes
# ==========================================
echo -e "\n${BLUE}[Step 6] Applying Routes${NC}"
echo "-------------------------------------------"

# 6.1 HTTPRoute
echo -e "\n${GREEN}>>> HTTPRoute${NC}"
apply_pattern "HTTPRoute_*.yaml" "$CONF_DIR"
apply_pattern "httproute_*.yaml" "$CONF_DIR"  # lowercase variants

# 6.2 GRPCRoute
echo -e "\n${GREEN}>>> GRPCRoute${NC}"
apply_pattern "GRPCRoute_*.yaml" "$CONF_DIR"

# 6.3 TCPRoute
echo -e "\n${GREEN}>>> TCPRoute${NC}"
apply_pattern "TCPRoute_*.yaml" "$CONF_DIR"

# 6.4 TLSRoute
echo -e "\n${GREEN}>>> TLSRoute${NC}"
apply_pattern "TLSRoute_*.yaml" "$CONF_DIR"

# 6.5 UDPRoute
echo -e "\n${GREEN}>>> UDPRoute${NC}"
apply_pattern "UDPRoute_*.yaml" "$CONF_DIR"

# ==========================================
# Summary
# ==========================================
echo -e "\n${BLUE}==========================================${NC}"
echo -e "${BLUE}                 Summary${NC}"
echo -e "${BLUE}==========================================${NC}"

if [ -n "$DRY_RUN" ]; then
    echo -e "${YELLOW}[DRY-RUN]${NC} No changes were made"
elif [ "$DELETE_MODE" = true ]; then
    echo -e "${GREEN}[DONE]${NC} Resources deleted successfully"
else
    echo -e "${GREEN}[DONE]${NC} Resources applied successfully"
fi

echo ""
echo "Note: EndpointSlice resources were NOT applied."
echo "      They should be auto-generated based on your actual backend services."
echo ""
echo "To verify resources:"
echo "  kubectl get all -n edgion-default"
echo "  kubectl get all -n edgion-test"
echo "  kubectl get gateways,httproutes,tcproutes -A"
echo ""
