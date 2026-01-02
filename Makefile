# Edgion Docker Image Build Makefile
# 
# Usage:
#   make build-gateway          # Build Gateway image
#   make build-controller       # Build Controller image
#   make build-ctl              # Build CLI tool image
#   make build-all              # Build all images
#   make push-all               # Push all images to registry
#
# Customization:
#   make build-gateway VERSION=1.0.0 IMAGE_REGISTRY=myregistry.com

# Configuration Variables
IMAGE_REGISTRY ?= docker.io
IMAGE_NAMESPACE ?= edgion
VERSION ?= 0.1.0
RUST_VERSION ?= 1.92
FEATURES ?= default

# Derived variables
GATEWAY_IMAGE := $(IMAGE_REGISTRY)/$(IMAGE_NAMESPACE)/edgion-gateway:$(VERSION)
CONTROLLER_IMAGE := $(IMAGE_REGISTRY)/$(IMAGE_NAMESPACE)/edgion-controller:$(VERSION)
CTL_IMAGE := $(IMAGE_REGISTRY)/$(IMAGE_NAMESPACE)/edgion-ctl:$(VERSION)

GATEWAY_IMAGE_LATEST := $(IMAGE_REGISTRY)/$(IMAGE_NAMESPACE)/edgion-gateway:latest
CONTROLLER_IMAGE_LATEST := $(IMAGE_REGISTRY)/$(IMAGE_NAMESPACE)/edgion-controller:latest
CTL_IMAGE_LATEST := $(IMAGE_REGISTRY)/$(IMAGE_NAMESPACE)/edgion-ctl:latest

# Docker build common flags
DOCKER_BUILD_FLAGS := --build-arg RUST_VERSION=$(RUST_VERSION) --build-arg FEATURES=$(FEATURES)
DOCKER_BUILDX_PLATFORMS := linux/amd64

# Colors for output
RED := \033[0;31m
GREEN := \033[0;32m
YELLOW := \033[0;33m
BLUE := \033[0;34m
NC := \033[0m # No Color

.PHONY: help
help: ## Show this help message
	@echo "$(BLUE)Edgion Docker Image Build$(NC)"
	@echo ""
	@echo "$(GREEN)Available targets:$(NC)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  $(YELLOW)%-20s$(NC) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(GREEN)Configuration:$(NC)"
	@echo "  IMAGE_REGISTRY   = $(YELLOW)$(IMAGE_REGISTRY)$(NC)"
	@echo "  IMAGE_NAMESPACE  = $(YELLOW)$(IMAGE_NAMESPACE)$(NC)"
	@echo "  VERSION          = $(YELLOW)$(VERSION)$(NC)"
	@echo "  RUST_VERSION     = $(YELLOW)$(RUST_VERSION)$(NC)"
	@echo "  FEATURES         = $(YELLOW)$(FEATURES)$(NC)"

.PHONY: check-docker
check-docker: ## Check if Docker is available
	@which docker > /dev/null || (echo "$(RED)Error: Docker not found. Please install Docker.$(NC)" && exit 1)
	@docker version > /dev/null 2>&1 || (echo "$(RED)Error: Docker daemon not running.$(NC)" && exit 1)
	@echo "$(GREEN)✓ Docker is available$(NC)"

.PHONY: build-gateway
build-gateway: check-docker ## Build Gateway image
	@echo "$(BLUE)Building Gateway image...$(NC)"
	docker build $(DOCKER_BUILD_FLAGS) \
		--build-arg BINARY=edgion-gateway \
		-t $(GATEWAY_IMAGE) \
		-t $(GATEWAY_IMAGE_LATEST) \
		-f Dockerfile .
	@echo "$(GREEN)✓ Gateway image built: $(GATEWAY_IMAGE)$(NC)"

.PHONY: build-controller
build-controller: check-docker ## Build Controller image
	@echo "$(BLUE)Building Controller image...$(NC)"
	docker build $(DOCKER_BUILD_FLAGS) \
		--build-arg BINARY=edgion-controller \
		-t $(CONTROLLER_IMAGE) \
		-t $(CONTROLLER_IMAGE_LATEST) \
		-f Dockerfile .
	@echo "$(GREEN)✓ Controller image built: $(CONTROLLER_IMAGE)$(NC)"

.PHONY: build-ctl
build-ctl: check-docker ## Build CLI tool image
	@echo "$(BLUE)Building CLI tool image...$(NC)"
	docker build $(DOCKER_BUILD_FLAGS) \
		--build-arg BINARY=edgion-ctl \
		-t $(CTL_IMAGE) \
		-t $(CTL_IMAGE_LATEST) \
		-f Dockerfile .
	@echo "$(GREEN)✓ CLI tool image built: $(CTL_IMAGE)$(NC)"

.PHONY: build-all
build-all: build-gateway build-controller build-ctl ## Build all images
	@echo "$(GREEN)✓ All images built successfully!$(NC)"

.PHONY: push-gateway
push-gateway: ## Push Gateway image to registry
	@echo "$(BLUE)Pushing Gateway image...$(NC)"
	docker push $(GATEWAY_IMAGE)
	docker push $(GATEWAY_IMAGE_LATEST)
	@echo "$(GREEN)✓ Gateway image pushed$(NC)"

.PHONY: push-controller
push-controller: ## Push Controller image to registry
	@echo "$(BLUE)Pushing Controller image...$(NC)"
	docker push $(CONTROLLER_IMAGE)
	docker push $(CONTROLLER_IMAGE_LATEST)
	@echo "$(GREEN)✓ Controller image pushed$(NC)"

.PHONY: push-ctl
push-ctl: ## Push CLI tool image to registry
	@echo "$(BLUE)Pushing CLI tool image...$(NC)"
	docker push $(CTL_IMAGE)
	docker push $(CTL_IMAGE_LATEST)
	@echo "$(GREEN)✓ CLI tool image pushed$(NC)"

.PHONY: push-all
push-all: push-gateway push-controller push-ctl ## Push all images to registry
	@echo "$(GREEN)✓ All images pushed successfully!$(NC)"

.PHONY: build-and-push-all
build-and-push-all: build-all push-all ## Build and push all images
	@echo "$(GREEN)✓ Build and push completed!$(NC)"

.PHONY: buildx-setup
buildx-setup: ## Setup Docker Buildx for multi-platform builds
	@echo "$(BLUE)Setting up Docker Buildx...$(NC)"
	docker buildx create --name edgion-builder --use || true
	docker buildx inspect --bootstrap
	@echo "$(GREEN)✓ Buildx ready$(NC)"

.PHONY: buildx-gateway
buildx-gateway: buildx-setup ## Build multi-platform Gateway image
	@echo "$(BLUE)Building multi-platform Gateway image...$(NC)"
	docker buildx build $(DOCKER_BUILD_FLAGS) \
		--platform $(DOCKER_BUILDX_PLATFORMS) \
		--build-arg BINARY=edgion-gateway \
		-t $(GATEWAY_IMAGE) \
		-t $(GATEWAY_IMAGE_LATEST) \
		--push \
		-f Dockerfile .
	@echo "$(GREEN)✓ Multi-platform Gateway image built and pushed$(NC)"

.PHONY: buildx-controller
buildx-controller: buildx-setup ## Build multi-platform Controller image
	@echo "$(BLUE)Building multi-platform Controller image...$(NC)"
	docker buildx build $(DOCKER_BUILD_FLAGS) \
		--platform $(DOCKER_BUILDX_PLATFORMS) \
		--build-arg BINARY=edgion-controller \
		-t $(CONTROLLER_IMAGE) \
		-t $(CONTROLLER_IMAGE_LATEST) \
		--push \
		-f Dockerfile .
	@echo "$(GREEN)✓ Multi-platform Controller image built and pushed$(NC)"

.PHONY: buildx-all
buildx-all: buildx-gateway buildx-controller ## Build and push all multi-platform images
	@echo "$(GREEN)✓ All multi-platform images built and pushed!$(NC)"

.PHONY: test-gateway
test-gateway: ## Test Gateway image
	@echo "$(BLUE)Testing Gateway image...$(NC)"
	@docker run --rm $(GATEWAY_IMAGE) /usr/local/bin/edgion-gateway --version || true
	@echo "$(GREEN)✓ Gateway image test completed$(NC)"

.PHONY: test-controller
test-controller: ## Test Controller image
	@echo "$(BLUE)Testing Controller image...$(NC)"
	@docker run --rm $(CONTROLLER_IMAGE) /usr/local/bin/edgion-controller --version || true
	@echo "$(GREEN)✓ Controller image test completed$(NC)"

.PHONY: list-images
list-images: ## List built Edgion images
	@echo "$(BLUE)Edgion Docker Images:$(NC)"
	@docker images | grep -E "(REPOSITORY|edgion)" || echo "$(YELLOW)No images found$(NC)"

.PHONY: clean-images
clean-images: ## Remove all Edgion images
	@echo "$(YELLOW)Removing Edgion images...$(NC)"
	@docker images -q "$(IMAGE_NAMESPACE)/*" | xargs -r docker rmi -f || true
	@echo "$(GREEN)✓ Images cleaned$(NC)"

.PHONY: version
version: ## Show current version
	@echo "$(BLUE)Version:$(NC) $(YELLOW)$(VERSION)$(NC)"

.DEFAULT_GOAL := help

