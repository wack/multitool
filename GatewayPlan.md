# Kubernetes Gateway API - CORE Conformance Implementation Plan

## Overview

This document outlines the complete implementation plan for a minimal Kubernetes Gateway API implementation that achieves CORE conformance level. The implementation will be built as part of the MultiTool CLI behind the `gateway` Cargo feature flag.

### Conformance Level

**Target**: CORE conformance only - no optional or extended features will be implemented.

### Core Resources

The following Kubernetes Gateway API resources are required for CORE conformance:

1. **GatewayClass** - Defines the class of Gateway infrastructure
2. **Gateway** - Provisions and configures load balancing infrastructure
3. **HTTPRoute** - Routes HTTP traffic from Gateway listeners to backend services
4. **ReferenceGrant** - Enables secure cross-namespace references (CORE requirement per reference.md:922)

---

## Implementation Tickets

### Phase 1: Development Environment and Foundation

#### Ticket 1.1: Local Development and Testing Environment

**Description**: Set up a local Kubernetes development environment using Kind with hot-reload capability for iterative conformance testing.

**Requirements**:

##### Kind Cluster Configuration
- Create Kind cluster configuration with extra mounts for source code
- Mount local workspace directory to cluster nodes for live code updates
- Configure cluster with necessary networking for Gateway testing

##### Controller Dockerfile
- Create Dockerfile based on `rust:1.90-trixie` image
- Install bacon for automatic rebuild on source changes
- Configure container to watch mounted source directory
- Set up automatic controller restart on rebuild

##### Hot-Reload Development Workflow
- Use bacon to watch for Rust source code changes
- Automatically rebuild controller binary when changes detected
- Restart controller process after successful rebuild
- Stream controller logs for debugging

##### Development Environment Setup Steps

**Complete setup process**:

1. **Install Prerequisites**
   - Install Docker
   - Install Kind: `go install sigs.k8s.io/kind@latest` or use package manager
   - Install kubectl

2. **Create Kind Configuration File** (`kind-config.yaml`):
   ```yaml
   kind: Cluster
   apiVersion: kind.x-k8s.io/v1alpha4
   nodes:
   - role: control-plane
     extraMounts:
     - hostPath: /path/to/multitool/workspace
       containerPath: /workspace
       readOnly: false
     - hostPath: /path/to/multitool/target
       containerPath: /target
       readOnly: false
   ```

3. **Create Controller Dockerfile** (`gateway/Dockerfile.dev`):
   ```dockerfile
   FROM rust:1.90-trixie

   # Install system dependencies
   RUN apt-get update && apt-get install -y \
       pkg-config \
       libssl-dev \
       && rm -rf /var/lib/apt/lists/*

   # Install bacon for hot-reload
   RUN cargo install bacon

   # Set working directory
   WORKDIR /workspace

   # Copy bacon configuration if exists, or create default
   COPY bacon.toml /workspace/bacon.toml 2>/dev/null || \
        echo '[jobs.gateway]\ncommand = ["cargo", "build", "--features", "gateway", "--bin", "multi"]\nwatch = ["src/", "Cargo.toml"]' > /workspace/bacon.toml

   # Expose any necessary ports (adjust as needed)
   EXPOSE 8080

   # Default command runs bacon in watch mode for gateway feature
   CMD ["bash", "-c", "bacon gateway || cargo build --features gateway && ./target/debug/multi gateway run"]
   ```

4. **Create Bacon Configuration** (`bacon.toml`):
   ```toml
   [jobs.gateway]
   command = [
       "cargo", "build",
       "--features", "gateway",
       "--bin", "multi"
   ]
   watch = ["src/", "Cargo.toml"]

   [jobs.gateway-check]
   command = [
       "cargo", "check",
       "--features", "gateway"
   ]
   watch = ["src/", "Cargo.toml"]

   [jobs.gateway-run]
   command = [
       "cargo", "run",
       "--features", "gateway",
       "--bin", "multi",
       "--",
       "gateway", "run"
   ]
   watch = ["src/", "Cargo.toml"]
   need_stdout = true
   ```

5. **Create Development Setup Script** (`scripts/dev-setup.sh`):
   ```bash
   #!/bin/bash
   set -e

   echo "Creating Kind cluster for Gateway development..."

   # Get absolute path to workspace
   WORKSPACE_DIR=$(pwd)
   TARGET_DIR="$WORKSPACE_DIR/target"

   # Create target directory if it doesn't exist
   mkdir -p "$TARGET_DIR"

   # Create Kind config with correct paths
   cat > kind-config.yaml <<EOF
   kind: Cluster
   apiVersion: kind.x-k8s.io/v1alpha4
   nodes:
   - role: control-plane
     extraMounts:
     - hostPath: $WORKSPACE_DIR
       containerPath: /workspace
       readOnly: false
     - hostPath: $TARGET_DIR
       containerPath: /target
       readOnly: false
   EOF

   # Create cluster
   kind create cluster --name gateway-dev --config kind-config.yaml

   # Build controller Docker image
   docker build -t multitool-gateway-dev:latest -f gateway/Dockerfile.dev .

   # Load image into Kind cluster
   kind load docker-image multitool-gateway-dev:latest --name gateway-dev

   echo "Kind cluster created successfully!"
   echo "Run 'kubectl cluster-info' to verify cluster is running"
   ```

6. **Create Controller Deployment Manifest** (`gateway/k8s/controller-deployment.yaml`):
   ```yaml
   apiVersion: v1
   kind: Namespace
   metadata:
     name: gateway-system
   ---
   apiVersion: v1
   kind: ServiceAccount
   metadata:
     name: gateway-controller
     namespace: gateway-system
   ---
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRole
   metadata:
     name: gateway-controller
   rules:
   - apiGroups: ["gateway.networking.k8s.io"]
     resources: ["gatewayclasses", "gateways", "httproutes", "referencegrants"]
     verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
   - apiGroups: ["gateway.networking.k8s.io"]
     resources: ["gatewayclasses/status", "gateways/status", "httproutes/status"]
     verbs: ["get", "update", "patch"]
   - apiGroups: [""]
     resources: ["services", "secrets"]
     verbs: ["get", "list", "watch"]
   - apiGroups: [""]
     resources: ["events"]
     verbs: ["create", "patch"]
   ---
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRoleBinding
   metadata:
     name: gateway-controller
   roleRef:
     apiGroup: rbac.authorization.k8s.io
     kind: ClusterRole
     name: gateway-controller
   subjects:
   - kind: ServiceAccount
     name: gateway-controller
     namespace: gateway-system
   ---
   apiVersion: apps/v1
   kind: Deployment
   metadata:
     name: gateway-controller
     namespace: gateway-system
   spec:
     replicas: 1
     selector:
       matchLabels:
         app: gateway-controller
     template:
       metadata:
         labels:
           app: gateway-controller
       spec:
         serviceAccountName: gateway-controller
         containers:
         - name: controller
           image: multitool-gateway-dev:latest
           imagePullPolicy: Never
           volumeMounts:
           - name: workspace
             mountPath: /workspace
           - name: target
             mountPath: /target
           env:
           - name: RUST_LOG
             value: "info,multi=debug"
         volumes:
         - name: workspace
           hostPath:
             path: /workspace
             type: Directory
         - name: target
           hostPath:
             path: /target
             type: Directory
   ```

7. **Create Quick Development Commands Script** (`scripts/dev-commands.sh`):
   ```bash
   #!/bin/bash

   # Quick reference commands for development

   function dev-logs() {
       echo "Streaming controller logs..."
       kubectl logs -f -n gateway-system deployment/gateway-controller
   }

   function dev-restart() {
       echo "Restarting controller..."
       kubectl rollout restart -n gateway-system deployment/gateway-controller
   }

   function dev-rebuild() {
       echo "Rebuilding and reloading controller image..."
       docker build -t multitool-gateway-dev:latest -f gateway/Dockerfile.dev .
       kind load docker-image multitool-gateway-dev:latest --name gateway-dev
       dev-restart
   }

   function dev-shell() {
       echo "Opening shell in controller pod..."
       kubectl exec -it -n gateway-system deployment/gateway-controller -- /bin/bash
   }

   function dev-status() {
       echo "Checking controller status..."
       kubectl get pods -n gateway-system
       kubectl get gatewayclasses
       kubectl get gateways --all-namespaces
       kubectl get httproutes --all-namespaces
   }

   # Show available commands
   echo "Available development commands:"
   echo "  dev-logs      - Stream controller logs"
   echo "  dev-restart   - Restart controller"
   echo "  dev-rebuild   - Rebuild and reload controller image"
   echo "  dev-shell     - Open shell in controller pod"
   echo "  dev-status    - Check controller and Gateway resources status"
   ```

8. **Initial Cluster Setup**:
   ```bash
   # Run setup script
   chmod +x scripts/dev-setup.sh
   ./scripts/dev-setup.sh

   # Install Gateway API CRDs
   kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.0.0/standard-install.yaml

   # Deploy controller
   kubectl apply -f gateway/k8s/controller-deployment.yaml

   # Source development commands
   source scripts/dev-commands.sh

   # Check status
   dev-status

   # Stream logs
   dev-logs
   ```

##### Iterative Testing Workflow

**Hot-reload development cycle**:

1. Make changes to controller source code in `src/gateway/` or related modules
2. Bacon automatically detects changes and rebuilds (inside container)
3. Controller process automatically restarts with new binary
4. Test changes immediately using kubectl to create/modify Gateway resources
5. View logs with `dev-logs` to debug
6. Repeat

**Manual rebuild cycle** (if bacon not working):
1. Make source code changes
2. Run `dev-rebuild` to rebuild and reload
3. View logs with `dev-logs`
4. Test with kubectl
5. Repeat

**Testing specific scenarios**:
```bash
# Create a test GatewayClass
cat <<EOF | kubectl apply -f -
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: multitool
spec:
  controllerName: multitool.run/gateway-controller
EOF

# Watch GatewayClass status
kubectl get gatewayclass multitool -o yaml

# Create a test Gateway
cat <<EOF | kubectl apply -f -
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-gateway
  namespace: default
spec:
  gatewayClassName: multitool
  listeners:
  - name: http
    protocol: HTTP
    port: 80
EOF

# Watch Gateway status
kubectl get gateway test-gateway -o yaml

# View controller logs for debugging
dev-logs
```

**Cleanup**:
```bash
# Delete Kind cluster
kind delete cluster --name gateway-dev

# Clean up generated files
rm -f kind-config.yaml
```

**Dependencies**: None

**Acceptance Criteria**:
- Kind cluster runs successfully with mounted volumes
- Controller builds and runs in cluster
- Source code changes trigger automatic rebuild via bacon
- Controller automatically restarts after successful rebuild
- Can create and test GatewayClass, Gateway, and HTTPRoute resources
- Logs are accessible for debugging
- Development iteration cycle is < 30 seconds from code change to running controller

---

#### Ticket 1.2: Kubernetes Client Integration

**Description**: Set up Kubernetes client integration to interact with the Kubernetes API server.

**Requirements**:
- Create Rust client for Kubernetes API using `kube-rs` or equivalent
- Implement authentication and authorization handling
- Add configuration for connecting to Kubernetes cluster
- Support standard kubeconfig file locations

**Dependencies**: Ticket 1.1

**Acceptance Criteria**:
- Can successfully connect to a Kubernetes cluster
- Can read and write custom resources
- Properly handles authentication errors

---

#### Ticket 1.3: GatewayClass CRD Definition

**Description**: Define and register the GatewayClass Custom Resource Definition.

**Requirements**:
- Define GatewayClass CRD schema matching Gateway API v1 specification
- Implement `spec.controllerName` field (reference.md:186-190)
- Implement `spec.parametersRef` field for controller parameters (reference.md:111-137)
- Implement `status.conditions` field for validation status (reference.md:139-182)

**Specification References**:
- GatewayClass overview: reference.md:69-208
- Controller selection: reference.md:184-204
- Parameters: reference.md:109-137

**Dependencies**: Ticket 1.2

**Acceptance Criteria**:
- GatewayClass CRD is installable in Kubernetes
- All required fields are present in the schema
- Validation rules are enforced

---

#### Ticket 1.4: GatewayClass Controller and Validation

**Description**: Implement GatewayClass controller that validates and manages GatewayClass resources.

**Requirements**:
- **MUST** validate GatewayClass parameters (reference.md:141)
  - "GatewayClasses MUST be validated by the provider to ensure that the configured parameters are valid"
- Set `Accepted` condition to `False` initially (reference.md:155-157)
- Set `Accepted` condition to `True` when configuration is valid (reference.md:159-168)
- Set `Accepted` condition to `False` with error details when invalid (reference.md:173-182)
- **RECOMMENDED** use unique controller name under administrative control (reference.md:192-195)

**Specification References**:
- GatewayClass status: reference.md:139-182
- Controller selection: reference.md:184-204

**Dependencies**: Ticket 1.3

**Acceptance Criteria**:
- Controller watches GatewayClass resources
- Validates GatewayClass configurations
- Updates status.conditions appropriately
- Handles invalid configurations with clear error messages

---

### Phase 2: Gateway Resource

#### Ticket 2.1: Gateway CRD Definition

**Description**: Define and register the Gateway Custom Resource Definition.

**Requirements**:
- Define Gateway CRD schema matching Gateway API v1 specification
- Implement `spec.gatewayClassName` field referencing GatewayClass (reference.md:32-33)
- Implement `spec.listeners` array for listener configuration (reference.md:34-35)
- Implement `spec.addresses` array for network address requests (reference.md:36)
- Implement `status.addresses` for actual bound addresses (reference.md:60-61)
- Implement `status.listeners` for listener status (reference.md:62)
- Implement `status.conditions` for Gateway status (reference.md:63)

**Specification References**:
- Gateway overview: reference.md:21-68
- Gateway spec: reference.md:30-36
- Gateway status: reference.md:56-67

**Dependencies**: Ticket 1.4

**Acceptance Criteria**:
- Gateway CRD is installable in Kubernetes
- All required spec and status fields are present
- Schema validation is in place

---

#### Ticket 2.2: Gateway Controller - Infrastructure Provisioning

**Description**: Implement Gateway controller that provisions load balancing infrastructure.

**Requirements**:
- Watch for Gateway resource creation/updates/deletion
- Validate that referenced GatewayClass exists and is accepted
- Provision load balancing infrastructure according to deployment model (reference.md:41-53)
- Bind network addresses and update `status.addresses` (reference.md:60-61)
- Update `status.conditions` to reflect Gateway state (reference.md:63-67)
- Handle errors and report via status conditions (reference.md:38-39)

**Specification References**:
- Deployment models: reference.md:41-53
- Gateway status: reference.md:56-67

**Dependencies**: Ticket 2.1

**Acceptance Criteria**:
- Gateway resources trigger infrastructure provisioning
- Status is updated with actual addresses
- Error conditions are properly reported
- Infrastructure is cleaned up on Gateway deletion

---

#### Ticket 2.3: Gateway Listener Configuration

**Description**: Implement listener configuration and status management for Gateway.

**Requirements**:
- Parse and validate listener configurations from `spec.listeners`
- Support listener fields: name, hostname, port, protocol, TLS settings
- Determine which routes can attach to each listener (reference.md:34-35)
- Implement built-in handshake mechanism for cross-namespace Route->Gateway binding (reference.md:894-900)
  - Configure which route kinds and namespaces are allowed per listener
  - This replaces ReferenceGrant for Route->Gateway binding
- Update `status.listeners` with per-listener status (reference.md:62)
- Validate listener combinations and conflicts
- Support listener section names for route attachment (reference.md:256-279)

**Specification References**:
- Listeners: reference.md:34-35
- Listener status: reference.md:62
- Section names: reference.md:256-279

**Dependencies**: Ticket 2.2

**Acceptance Criteria**:
- Listeners are configured according to spec
- Listener status is accurately reported
- Route attachment rules are enforced per listener
- Listener conflicts are detected and reported

---

### Phase 3: HTTPRoute Resource

#### Ticket 3.1: HTTPRoute CRD Definition

**Description**: Define and register the HTTPRoute Custom Resource Definition.

**Requirements**:
- Define HTTPRoute CRD schema matching Gateway API v1 specification
- Implement `spec.parentRefs` for Gateway attachment (reference.md:217-218)
- Implement `spec.hostnames` array (optional) (reference.md:220-221)
- Implement `spec.rules` array for routing rules (reference.md:222-225)
- Implement `status.parents` for route status per parent Gateway (reference.md:475-501)

**Specification References**:
- HTTPRoute overview: reference.md:209-520
- HTTPRoute spec: reference.md:214-225
- HTTPRoute status: reference.md:467-501

**Dependencies**: Ticket 2.3

**Acceptance Criteria**:
- HTTPRoute CRD is installable in Kubernetes
- All required fields are present in schema
- Validation rules are enforced

---

#### Ticket 3.2: HTTPRoute Controller - Parent Reference Resolution

**Description**: Implement HTTPRoute controller that resolves parent Gateway references and updates status.

**Requirements**:
- Watch for HTTPRoute resource creation/updates/deletion
- Resolve `parentRefs` to actual Gateway resources (reference.md:230-248)
- Support binding to specific Gateway listeners via `sectionName` (reference.md:271-279)
- Support binding to Gateway ports via `port` field (reference.md:281-307)
- Validate that target Gateway allows HTTPRoute attachment from route's namespace
- Update `status.parents` with acceptance status per Gateway (reference.md:484-501)
- Set `Accepted` condition for each parent Gateway (reference.md:499-500)

**Specification References**:
- Attaching to Gateways: reference.md:230-307
- RouteStatus parents: reference.md:475-501

**Dependencies**: Ticket 3.1

**Acceptance Criteria**:
- HTTPRoute can attach to Gateway resources
- Parent references are correctly resolved
- Status reflects attachment state per parent
- Namespace restrictions are enforced

---

#### Ticket 3.3: HTTPRoute Hostname Matching

**Description**: Implement hostname-based routing for HTTPRoute.

**Requirements**:
- Parse `hostnames` field from HTTPRoute spec (reference.md:309-334)
- Match against Host header of HTTP requests (reference.md:311-313)
- Support fully qualified domain names as per RFC 3986 (reference.md:314-318)
- Reject IP addresses in hostnames (reference.md:317)
- Reject port numbers in hostnames (reference.md:318)
- Route based on rules when no hostname specified (reference.md:321-322)
- Match hostnames before evaluating rules (reference.md:320-321)

**Specification References**:
- Hostnames: reference.md:309-334
- RFC 3986 compliance: reference.md:314-318

**Dependencies**: Ticket 3.2

**Acceptance Criteria**:
- Hostname matching works correctly
- RFC 3986 compliance is enforced
- Requests are routed based on hostname matches
- Default behavior (no hostname) works correctly

---

#### Ticket 3.4: HTTPRoute Rules - Match Conditions

**Description**: Implement HTTP request matching logic for HTTPRoute rules.

**Requirements**:
- Support multiple matches per rule (reference.md:344-372)
- Treat each match as independent (OR logic) (reference.md:345-346)
- Implement path matching (reference.md:356-357, 361-362)
- Implement header matching (reference.md:358-360)
- Implement query parameter matching (reference.md:412-413)
- Implement HTTP method matching (reference.md:417)
- Default to prefix path match on "/" when no matches specified (reference.md:371-372)
- Match conditions within a single match use AND logic (reference.md:365-369)

**Specification References**:
- Rules overview: reference.md:336-340
- Matches: reference.md:342-372
- Match examples: reference.md:348-369, 410-418

**Dependencies**: Ticket 3.3

**Acceptance Criteria**:
- Path, header, query, and method matching all work
- Match logic (OR between matches, AND within a match) is correct
- Default match behavior is implemented
- Complex match combinations work correctly

---

#### Ticket 3.5: HTTPRoute Rules - Core Filters

**Description**: Implement all CORE filters for HTTPRoute.

**Requirements**:
- **MUST** support all "core" filters (reference.md:388)
  - "All 'core' filters MUST be supported by implementations"
- Implement filter execution in request/response lifecycle (reference.md:376-380)
- Handle filter ordering (currently unspecified, reference.md:382-384)
- Validate filter compatibility (reference.md:395-401)
- **must** clearly document any unsupported filter combinations (reference.md:397-398)
  - "implementation cannot support other combinations of filters, they must clearly document that limitation"
- Set `Accepted` condition to `False` with `IncompatibleFilters` reason when incompatible (reference.md:399-401)
- Note: RequestRedirect and URLRewrite filters cannot be combined (reference.md:396-398)

**Specification References**:
- Filters overview: reference.md:374-401
- Conformance requirement: reference.md:386-390

**Note**: The reference document does not list which specific filters are "core" vs "extended". This will need to be determined from the full Gateway API specification.

**Dependencies**: Ticket 3.4

**Acceptance Criteria**:
- All core filters are implemented
- Filter incompatibilities are detected and reported
- Filters execute at correct points in lifecycle
- Status conditions reflect filter configuration issues

---

#### Ticket 3.6: HTTPRoute Rules - Backend References

**Description**: Implement backend reference resolution and traffic forwarding for HTTPRoute.

**Requirements**:
- Parse `backendRefs` from rule (reference.md:403-432)
- Support forwarding to Kubernetes Services (reference.md:410-419, 425-429)
- Support `weight` field for traffic splitting (reference.md:425-429)
- Handle empty backendRefs (no forwarding) (reference.md:406-408)
- Return 404 when no backendRefs and no response-generating filters (reference.md:406-408)
- Support Service port references (reference.md:411, 419)
- Support backend protocol via Service `appProtocol` field (reference.md:460-465)

**Specification References**:
- BackendRefs overview: reference.md:403-432
- Backend protocol: reference.md:460-465

**Dependencies**: Ticket 3.5

**Acceptance Criteria**:
- Traffic is correctly forwarded to backend Services
- Weight-based traffic splitting works
- 404 is returned when appropriate
- Multiple backends per rule are supported
- Backend protocols are respected

---

#### Ticket 3.7: HTTPRoute Rules - Timeouts

**Description**: Implement timeout handling for HTTPRoute rules.

**Requirements**:
- Support `request` timeout for full request-response transaction (reference.md:440-441)
- Support `backendRequest` timeout for individual backend requests (reference.md:442-443)
- **MUST** interpret zero-valued timeout ("0s") as disabling timeout (reference.md:446)
- **MUST** enforce valid non-zero timeout >= 1ms (reference.md:446)
- Validate that `backendRequest` <= `request` timeout (reference.md:444-445)
- Handle unspecified timeouts as implementation-specific (reference.md:436)

**Specification References**:
- Timeouts: reference.md:434-450
- Timeout validation: reference.md:444-446

**Dependencies**: Ticket 3.6

**Acceptance Criteria**:
- Request timeouts are enforced
- Backend request timeouts are enforced
- Zero-valued timeouts disable timeout behavior
- Timeout validation catches invalid configurations
- Timeout relationship (backendRequest <= request) is enforced

---

#### Ticket 3.8: HTTPRoute Merging and Conflict Resolution

**Description**: Implement proper merging behavior when multiple HTTPRoutes attach to a single Gateway.

**Requirements**:
- Ensure only one Route rule matches each request (reference.md:503-507)
- Implement conflict resolution as specified in HTTPRouteRule documentation (reference.md:507)
- Handle multiple HTTPRoutes attached to same Gateway (reference.md:503-507)

**Optional Field Note**:
- HTTPRoute rules include an optional `name` field (reference.md:452-458)
- If the `name` field is supported, its value **must** comply with the `SectionName` type (reference.md:456)
- Since this is optional and the implementation is minimal/CORE only, supporting the `name` field can be deferred or omitted

**Specification References**:
- Merging: reference.md:503-507
- Rule name field: reference.md:452-458

**Note**: The reference document points to the API specification for detailed conflict resolution rules.

**Dependencies**: Ticket 3.7

**Acceptance Criteria**:
- Multiple HTTPRoutes can attach to one Gateway
- Request matching is deterministic (only one rule matches)
- Conflicts are resolved according to specification
- Route precedence is correctly implemented

---

### Phase 4: ReferenceGrant (Cross-Namespace Support)

#### Ticket 4.1: ReferenceGrant CRD Definition

**Description**: Define and register the ReferenceGrant Custom Resource Definition.

**Requirements**:
- Define ReferenceGrant CRD schema matching Gateway API v1beta1 specification
- Implement `spec.from` array for source resource specifications (reference.md:813-814)
- Implement `spec.to` array for target resource specifications (reference.md:816-819)
- Support `from` fields: group, kind, namespace (reference.md:813-814)
- Support `to` fields: group, kind (reference.md:816-819)
- Note: namespace not needed in `to` list (reference.md:817-819)
- Each ReferenceGrant only supports single From and To section (reference.md:861-863)
- Additional trust relationships **must** be modeled with additional ReferenceGrant resources (reference.md:862-863)
- Resource names intentionally excluded from "From" section (reference.md:864-868)
- Single Namespace allowed per "From" struct (reference.md:869-870)
- ReferenceGrants have purely additive effect - they stack without conflict (reference.md:871-872)

**Specification References**:
- ReferenceGrant overview: reference.md:790-945
- Structure: reference.md:808-819
- API design decisions: reference.md:857-872
- Example: reference.md:821-855

**Dependencies**: Ticket 3.8

**Acceptance Criteria**:
- ReferenceGrant CRD is installable in Kubernetes
- From and To specifications work correctly
- Schema enforces single From and To section (reference.md:861-863)
- Multiple ReferenceGrants can be created for multiple trust relationships

---

#### Ticket 4.2: ReferenceGrant Controller - Runtime Verification

**Description**: Implement ReferenceGrant controller with runtime verification of cross-namespace references.

**Requirements**:
- **MUST** watch for changes to ReferenceGrant resources (reference.md:880)
  - "Implementations MUST watch for changes to these resources"
- **MUST** recalculate validity of cross-namespace references after each change/deletion (reference.md:880-882)
- **MUST NOT** expose information about resources in other namespaces without ReferenceGrant (reference.md:885-890)
  - "implementations MUST NOT expose information about the existence of a resource in another namespace unless a ReferenceGrant exists"
- Focus status on missing ReferenceGrant, not on resource existence (reference.md:887-890)
- Provide no hints about whether referenced resource exists (reference.md:890)

**Important Notes**:
- **Exception**: Cross-namespace Route -> Gateway binding does NOT use ReferenceGrant (reference.md:894-900)
  - This uses a built-in handshake mechanism in Gateway Listeners instead
- If making exceptions to ReferenceGrant, implementation **MUST** clearly document this and detail alternative safeguards (reference.md:907-910)
- **MUST** only make exceptions if absolutely certain other equally effective safeguards are in place (reference.md:916-918)
- Be careful to avoid confused deputy attacks (reference.md:914-916)

**Specification References**:
- Implementation guidelines: reference.md:878-890
- Exceptions: reference.md:892-918

**Dependencies**: Ticket 4.1

**Acceptance Criteria**:
- Controller watches ReferenceGrant resources
- Cross-namespace reference validity is recalculated on changes
- Status messages do not leak information about resources
- Security model is enforced
- Route->Gateway binding uses Gateway's built-in mechanism, not ReferenceGrant

---

#### Ticket 4.3: Cross-Namespace References in HTTPRoute

**Description**: Implement ReferenceGrant support for cross-namespace backend references in HTTPRoute.

**Requirements**:
- **MUST** use ReferenceGrant flow for cross-namespace references (reference.md:932-934)
  - "all implementations MUST use this flow for any cross namespace references in the Gateway and any of the core xRoute types"
- Validate ReferenceGrant exists before allowing cross-namespace backend reference (reference.md:823-840)
- Support HTTPRoute referencing Service in different namespace (reference.md:828-840)
- Require ReferenceGrant in target namespace allowing HTTPRoute -> Service reference (reference.md:842-855)
- Implement additive stacking of multiple ReferenceGrants (reference.md:871-872)

**Specification References**:
- Conformance requirement: reference.md:920-937
- HTTPRoute example: reference.md:823-855
- Additive behavior: reference.md:871-872

**Dependencies**: Ticket 4.2

**Acceptance Criteria**:
- HTTPRoute can reference backends in other namespaces with ReferenceGrant
- References fail without appropriate ReferenceGrant
- Multiple ReferenceGrants stack correctly
- Security model prevents unauthorized cross-namespace access

---

#### Ticket 4.4: Cross-Namespace References in Gateway

**Description**: Implement ReferenceGrant support for cross-namespace references from Gateway resources.

**Requirements**:
- **MUST** use ReferenceGrant for cross-namespace Secret references (reference.md:793-794, 932-934)
- Validate ReferenceGrant exists for Gateway -> Secret references
- Handle TLS certificate secrets in different namespaces
- Note: Gateway -> Route binding uses built-in handshake mechanism, not ReferenceGrant (reference.md:894-900)

**Specification References**:
- ReferenceGrant overview: reference.md:790-794
- Exceptions: reference.md:892-900
- Conformance requirement: reference.md:920-937

**Dependencies**: Ticket 4.3

**Acceptance Criteria**:
- Gateway can reference Secrets in other namespaces with ReferenceGrant
- References fail without appropriate ReferenceGrant
- Gateway-to-Route binding continues to use built-in mechanism
- Security model is maintained

---

### Phase 5: Integration and Testing

#### Ticket 5.1: Cargo Feature Flag Setup

**Description**: Configure the `gateway` Cargo feature flag and module structure.

**Requirements**:
- Add `gateway` feature flag to Cargo.toml
- Create module structure under feature gate
- Add conditional compilation attributes
- Configure dependencies needed only for Gateway (e.g., kube-rs)
- Ensure project builds with and without the feature flag

**Dependencies**: Ticket 4.4

**Acceptance Criteria**:
- `cargo build` works without `gateway` feature
- `cargo build --features gateway` builds Gateway implementation
- Code organization is clean and maintainable
- Feature-gated dependencies are properly configured

---

#### Ticket 5.2: CLI Integration

**Description**: Integrate Gateway controller into MultiTool CLI.

**Requirements**:
- Add Gateway-related CLI commands (e.g., `multi gateway run`)
- Implement controller lifecycle management
- Add configuration options for Kubernetes cluster connection
- Integrate with existing MultiTool architecture (subsystems model)
- Add logging and error handling consistent with MultiTool patterns

**Dependencies**: Ticket 5.1

**Acceptance Criteria**:
- Gateway controller can be started from CLI
- Controller integrates with MultiTool's subsystem architecture
- Logging is consistent with MultiTool conventions
- Configuration follows MultiTool patterns

---

#### Ticket 5.3: CORE Conformance Test Suite

**Description**: Implement and run Gateway API CORE conformance tests in the local Kind cluster.

**Requirements**:
- Set up Gateway API conformance test harness
- Run all CORE conformance tests for:
  - GatewayClass
  - Gateway
  - HTTPRoute
  - ReferenceGrant
- Document test results
- Fix any failing tests iteratively using the dev environment from Ticket 1.1
- Achieve 100% CORE conformance

**Testing Process**:
1. Use development environment from Ticket 1.1
2. Install Gateway API conformance test suite
3. Configure tests to target the MultiTool GatewayClass
4. Run conformance tests: `go test -v ./conformance/...`
5. Analyze failures and iterate:
   - Review test failure logs
   - Make code changes
   - Let bacon rebuild automatically
   - Re-run specific failing tests
   - Repeat until all tests pass
6. Document final conformance report

**Dependencies**: Ticket 5.2

**Acceptance Criteria**:
- All CORE conformance tests pass
- Test results are documented
- Any deviations from spec are identified and resolved
- CORE conformance level is achieved
- Conformance report is generated

---

#### Ticket 5.4: Documentation

**Description**: Create comprehensive documentation for the Gateway implementation.

**Requirements**:
- Document Gateway feature flag usage
- Document CLI commands for Gateway controller
- Document configuration options
- Create user guide for deploying and using the Gateway
- Document CORE conformance level and limitations (no extended features)
- Document architecture and design decisions
- Document local development environment setup (from Ticket 1.1)
- Update main MultiTool README with Gateway information

**Dependencies**: Ticket 5.3

**Acceptance Criteria**:
- Complete user documentation exists
- Architecture is documented
- CLI reference includes Gateway commands
- CORE conformance level is clearly documented
- Limitations and scope are clear

---

## Summary

### Total Tickets: 23

**Phase 1 (Development Environment and Foundation)**: 4 tickets
**Phase 2 (Gateway Resource)**: 3 tickets
**Phase 3 (HTTPRoute Resource)**: 8 tickets
**Phase 4 (ReferenceGrant)**: 4 tickets
**Phase 5 (Integration and Testing)**: 4 tickets

### Critical MUST Requirements Summary

1. **GatewayClass Validation** (reference.md:141): GatewayClasses MUST be validated
2. **Core Filters Support** (reference.md:388): All "core" filters MUST be supported for HTTPRoute
3. **Filter Documentation** (reference.md:397): Unsupported filter combinations must be clearly documented
4. **Timeout Handling** (reference.md:446): Zero-valued timeouts MUST disable timeout; non-zero MUST be >= 1ms
5. **ReferenceGrant Watching** (reference.md:880): MUST watch for changes to ReferenceGrant resources
6. **Information Hiding** (reference.md:885): MUST NOT expose resource existence across namespaces without ReferenceGrant
7. **Cross-Namespace Flow** (reference.md:932, 936): MUST use ReferenceGrant for all cross-namespace references
8. **Exception Documentation** (reference.md:907): If making exceptions to ReferenceGrant, MUST clearly document and detail alternative safeguards
9. **Safe Exceptions Only** (reference.md:916): Exceptions to ReferenceGrant MUST only be made with equally effective safeguards

### Dependency Flow

```
1.1 (Dev Environment)
  └─> 1.2 (K8s Client)
        └─> 1.3 (GatewayClass CRD)
              └─> 1.4 (GatewayClass Controller)
                    └─> 2.1 (Gateway CRD)
                          └─> 2.2 (Gateway Controller)
                                └─> 2.3 (Gateway Listeners)
                                      └─> 3.1 (HTTPRoute CRD)
                                            └─> 3.2 (HTTPRoute Controller)
                                                  └─> 3.3 (Hostname Matching)
                                                        └─> 3.4 (Match Conditions)
                                                              └─> 3.5 (Core Filters)
                                                                    └─> 3.6 (Backend Refs)
                                                                          └─> 3.7 (Timeouts)
                                                                                └─> 3.8 (Merging)
                                                                                      └─> 4.1 (ReferenceGrant CRD)
                                                                                            └─> 4.2 (ReferenceGrant Controller)
                                                                                                  └─> 4.3 (HTTPRoute Cross-NS)
                                                                                                        └─> 4.4 (Gateway Cross-NS)
                                                                                                              └─> 5.1 (Feature Flag)
                                                                                                                    └─> 5.2 (CLI Integration)
                                                                                                                          └─> 5.3 (Conformance Tests)
                                                                                                                                └─> 5.4 (Documentation)
```

### Notes

- All line number citations reference `/Users/robbie/workspace/wack/multitool/.claude/skills/gateway-api/reference.md`
- This plan focuses exclusively on CORE conformance level
- No extended or optional features are included
- The reference document notes that the specific list of "core" vs "extended" filters needs to be determined from the full API specification (see note in Ticket 3.5)
- Some conflict resolution details are referenced but not fully specified in the reference document (see note in Ticket 3.8)
