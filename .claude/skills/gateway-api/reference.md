# Kubernetes Gateway API Specification Reference

*This document will contain the complete reference documentation for the Kubernetes Gateway API specification, including all API types, conformance requirements, and implementation details.*

## API Types

### Core Resources

#### Gateway

A `Gateway` is 1:1 with the lifecycle of the configuration of infrastructure.
When a user creates a `Gateway`, some load balancing infrastructure is
provisioned or configured (see below for details) by the `GatewayClass`
controller. `Gateway` is the resource that triggers actions in this API. Other
resources in this API are configuration snippets until a Gateway has been
created to link the resources together.

The `Gateway` spec defines the following:

*   `GatewayClassName`- Defines the name of a `GatewayClass` object used by
    this Gateway.
*   `Listeners`-  Define the hostnames, ports, protocol, termination, TLS
    settings and which routes can be attached to a listener.
*   `Addresses`- Define the network addresses requested for this gateway.

If the desired configuration specified in Gateway spec cannot be achieved, the
Gateway will be in an error state with details provided by status conditions.

##### Deployment models

Depending on the `GatewayClass`, the creation of a `Gateway` could do any of
the following actions:

* Use cloud APIs to create an LB instance.
* Spawn a new instance of a software LB (in this or another cluster).
* Add a configuration stanza to an already instantiated LB to handle the new
  routes.
* Program the SDN to implement the configuration.
* Something else we haven't thought of yet...

The API does not specify which one of these actions will be taken.

##### Gateway Status

`GatewayStatus` is used to surface the status of a `Gateway` relative to the
desired state represented in `spec`. `GatewayStatus` consists of the following:

- `Addresses`- Lists the IP addresses that have actually been bound to the
  Gateway.
- `Listeners`- Provide status for each unique listener defined in `spec`.
- `Conditions`- Describe the current status conditions of the Gateway.

Both `Conditions` and `Listeners.conditions` follow the conditions pattern used
elsewhere in Kubernetes. This is a list that includes a type of condition, the
status of the condition and the last time this condition changed.

#### GatewayClass

??? success "Standard Channel since v0.5.0"

    The `GatewayClass` resource is GA and has been part of the Standard Channel since
    `v0.5.0`. For more information on release channels, refer to our [versioning
    guide](../concepts/versioning.md).

[GatewayClass][gatewayclass] is cluster-scoped resource defined by the
infrastructure provider. This resource represents a class of Gateways that can
be instantiated.

> Note: GatewayClass serves the same function as the
> [`networking.IngressClass` resource][ingress-class-api].

```yaml
kind: GatewayClass
metadata:
  name: cluster-gateway
spec:
  controllerName: "example.net/gateway-controller"
```

We expect that one or more `GatewayClasses` will be created by the
infrastructure provider for the user. It allows decoupling of which mechanism
(e.g. controller) implements the `Gateways` from the user. For instance, an
infrastructure provider may create two `GatewayClasses` named `internet` and
`private` to reflect `Gateways` that define Internet-facing vs private, internal
applications.

```yaml
kind: GatewayClass
metadata:
  name: internet
  ...
---
kind: GatewayClass
metadata:
  name: private
  ...
```

The user of the classes will not need to know *how* `internet` and `private` are
implemented. Instead, the user will only need to understand the resulting
properties of the class that the `Gateway` was created with.

##### GatewayClass parameters

Providers of the `Gateway` API may need to pass parameters to their controller
as part of the class definition. This is done using the
`GatewayClass.spec.parametersRef` field:

```yaml
# GatewayClass for Gateways that define Internet-facing applications.
kind: GatewayClass
metadata:
  name: internet
spec:
  controllerName: "example.net/gateway-controller"
  parametersRef:
    group: example.net
    kind: Config
    name: internet-gateway-config
---
apiVersion: example.net/v1alpha1
kind: Config
metadata:
  name: internet-gateway-config
spec:
  ip-address-pool: internet-vips
  ...
```

Using a Custom Resource for `GatewayClass.spec.parametersRef` is encouraged
but implementations may resort to using a ConfigMap if needed.

##### GatewayClass status

`GatewayClasses` MUST be validated by the provider to ensure that the configured
parameters are valid. The validity of the class will be signaled to the user via
`GatewayClass.status`:

```yaml
kind: GatewayClass
...
status:
  conditions:
  - type: Accepted
    status: False
    ...
```

A new `GatewayClass` will start with the `Accepted` condition set to
`False`. At this point the controller has not seen the configuration. Once the
controller has processed the configuration, the condition will be set to
`True`:

```yaml
kind: GatewayClass
...
status:
  conditions:
  - type: Accepted
    status: True
    ...
```

If there is an error in the `GatewayClass.spec`, the conditions will be
non-empty and contain information about the error.

```yaml
kind: GatewayClass
...
status:
  conditions:
  - type: Accepted
    status: False
    Reason: BadFooBar
    Message: "foobar" is an FooBar.
```

##### GatewayClass controller selection

The `GatewayClass.spec.controller` field determines the controller implementation
responsible for managing the `GatewayClass`. The format of the field is opaque
and specific to a particular controller. The GatewayClass selected by a given
controller field depends on how various controller(s) in the cluster interpret
this field.

It is RECOMMENDED that controller authors/deployments make their selection
unique by using a domain / path combination under their administrative control
(e.g. controller managing of all `controller`s starting with `example.net` is the
owner of the `example.net` domain) to avoid conflicts.

Controller versioning can be done by encoding the version of a controller into
the path portion. An example scheme could be (similar to container URIs):

```text
example.net/gateway/v1   // Use version 1
example.net/gateway/v2.1 // Use version 2.1
example.net/gateway      // Use the default version
```

[gatewayclass]: ../reference/spec.md#gateway.networking.k8s.io/v1.GatewayClass
[ingress-class-api]: https://kubernetes.io/docs/concepts/services-networking/ingress/#ingress-class

#### HTTPRoute

??? success "Standard Channel since v0.5.0"

    The `HTTPRoute` resource is GA and has been part of the Standard Channel since
    `v0.5.0`. For more information on release channels, refer to our [versioning
    guide](../concepts/versioning.md).

[HTTPRoute][httproute] is a Gateway API type for specifying routing behavior
of HTTP requests from a Gateway listener to an API object, i.e. Service.

##### Spec

The specification of an HTTPRoute consists of:

- [ParentRefs][parentRef]- Define which Gateways this Route wants to be attached
  to.
- [Hostnames][hostname] (optional)- Define a list of hostnames to use for
  matching the Host header of HTTP requests.
- [Rules][httprouterule]- Define a list of rules to perform actions against
  matching HTTP requests. Each rule consists of [matches][matches],
  [filters][filters] (optional), [backendRefs][backendRef] (optional),
  [timeouts][timeouts] (optional), and [name][sectionName] (optional) fields.

The following illustrates an HTTPRoute that sends all traffic to one Service:
![httproute-basic-example](../images/httproute-basic-example.svg)

###### Attaching to Gateways

Each Route includes a way to reference the parent resources it wants to attach
to. In most cases, that's going to be Gateways, but there is some flexibility
here for implementations to support other types of parent resources.

The following example shows how a Route would attach to the `acme-lb` Gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: httproute-example
spec:
  parentRefs:
  - name: acme-lb
```

Note that the target Gateway needs to allow HTTPRoutes from the route's
namespace to be attached for the attachment to be successful.

You can also attach routes to specific sections of the parent resource.
For example, let's say that the `acme-lb` Gateway includes the following
listeners:

```yaml
  listeners:
  - name: foo
    protocol: HTTP
    port: 8080
    ...
  - name: bar
    protocol: HTTP
    port: 8090
    ...
  - name: baz
    protocol: HTTP
    port: 8090
    ...
```

You can bind a route to listener `foo` only, using the `sectionName` field
in `parentRefs`:

```yaml
spec:
  parentRefs:
  - name: acme-lb
    sectionName: foo
```

Alternatively, you can achieve the same effect by using the `port` field,
instead of `sectionName`, in the `parentRefs`:

```yaml
spec:
  parentRefs:
  - name: acme-lb
    port: 8080
```

Binding to a port also allows you to attach to multiple listeners at once.
For example, binding to port `8090` of the `acme-lb` Gateway would be more
convenient than binding to the corresponding listeners by name:

```yaml
spec:
  parentRefs:
  - name: acme-lb
    sectionName: bar
  - name: acme-lb
    sectionName: baz
```

However, when binding Routes by port number, Gateway admins will no longer have
the flexibility to switch ports on the Gateway without also updating the Routes.
The approach should only be used when a Route should apply to a specific port
number as opposed to listeners whose ports may be changed.

###### Hostnames

Hostnames define a list of hostnames to match against the Host header of the
HTTP request. When a match occurs, the HTTPRoute is selected to perform request
routing based on rules and filters (optional). A hostname is the fully qualified
domain name of a network host, as defined by [RFC 3986][rfc-3986]. Note the
following deviations from the "host" part of the URI as defined in the RFC:

- IPs are not allowed.
- The : delimiter is not respected because ports are not allowed.

Incoming requests are matched against hostnames before the HTTPRoute rules are
evaluated. If no hostname is specified, traffic is routed based on HTTPRoute
rules and filters (optional).

The following example defines hostname "my.example.com":

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: httproute-example
spec:
  hostnames:
  - my.example.com
```

###### Rules

Rules define semantics for matching an HTTP request based on conditions,
optionally executing additional processing steps, and optionally forwarding
the request to an API object.

####### Matches

Matches define conditions used for matching an HTTP request. Each match is
independent, i.e. this rule will be matched if any single match is satisfied.

Take the following matches configuration as an example:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
...
spec:
  rules:
  - matches:
    - path:
        value: "/foo"
      headers:
      - name: "version"
        value: "2"
    - path:
        value: "/v2/foo"
```

For a request to match against this rule, it must satisfy EITHER of the
following conditions:

 - A path prefixed with /foo **AND** contains the header "version: 2"
 - A path prefix of /v2/foo

If no matches are specified, the default is a prefix path match on "/",
which has the effect of matching every HTTP request.

####### Filters (optional)

Filters define processing steps that must be completed during the request or
response lifecycle. Filters act as an extension point to express additional
processing that may be performed in Gateway implementations. Some examples
include request or response modification, implementing authentication
strategies, rate-limiting, and traffic shaping.

The following example adds header "my-header: foo" to HTTP requests with Host
header "my.filter.com".
```yaml
{% include 'standard/http-filter.yaml' %}
```

API conformance is defined based on the filter type. The effects of ordering
multiple behaviors is currently unspecified. This may change in the future
based on feedback during the alpha stage.

Conformance levels are defined by the filter type:

 - All "core" filters MUST be supported by implementations.
 - Implementers are encouraged to support "extended" filters.
 - "Implementation-specific" filters have no API guarantees across implementations.

Specifying a core filter multiple times has unspecified or
implementation-specific conformance.

All filters are expected to be compatible with each other except for the
URLRewrite and RequestRedirect filters, which may not be combined. If an
implementation cannot support other combinations of filters, they must clearly
document that limitation. In cases where incompatible or unsupported
filters are specified and cause the `Accepted` condition to be set to status
`False`, implementations may use the `IncompatibleFilters` reason to specify
this configuration error.

####### BackendRefs (optional)

BackendRefs defines API objects where matching requests should be sent. If
unspecified, the rule performs no forwarding. If unspecified and no filters
are specified that would result in a response being sent, a 404 error code
is returned.

The following example forwards HTTP requests for path prefix `/bar` to service
"my-service1" on port `8080`, and HTTP requests fulfilling _all_ four of the 
following criteria

- header `magic: foo` 
- query param `great: example`
- path prefix `/some/thing`
- method `GET`

to service "my-service2" on port `8080`:
```yaml
{% include 'standard/basic-http.yaml' %}
```

The following example uses the `weight` field to forward 90% of HTTP requests to
`foo.example.com` to the "foo-v1" Service and the other 10% to the "foo-v2"
Service:
```yaml
{% include 'standard/traffic-splitting/traffic-split-2.yaml' %}
```

Reference the [backendRef][backendRef] API documentation for additional details
on `weight` and other fields.

####### Timeouts (optional)

??? example "Experimental Channel since v1.0.0"

    HTTPRoute timeouts have been part of the Experimental Channel since `v1.0.0`.
    For more information on release channels, refer to our
    [versioning guide](../concepts/versioning.md).

HTTPRoute Rules include a `Timeouts` field. If unspecified, timeout behavior is implementation-specific.

There are 2 kinds of timeouts that can be configured in an HTTPRoute Rule:

1. `request` is the timeout for the Gateway API implementation to send a response to a client HTTP request. This timeout is intended to cover as close to the whole request-response transaction as possible, although an implementation MAY choose to start the timeout after the entire request stream has been received instead of immediately after the transaction is initiated by the client.

2. `backendRequest` is a timeout for a single request from the Gateway to a backend. This timeout covers the time from when the request first starts being sent from the gateway to when the full response has been received from the backend. This can be particularly helpful if the Gateway retries connections to a backend.

Because the `request` timeout encompasses the `backendRequest` timeout, the value of `backendRequest` must not be greater than the value of `request` timeout.

Timeouts are optional, and their fields are of type [Duration](../geps/gep-2257/index.md). A zero-valued timeout ("0s") MUST be interpreted as disabling the timeout. A valid non-zero-valued timeout MUST be >= 1ms.

The following example uses the `request` field which will cause a timeout if a client request is taking longer than 10 seconds to complete. The example also defines a 2s `backendRequest` which specifies a timeout for an individual request from the gateway to a backend service `timeout-svc`:

```yaml
{% include 'experimental/http-route-timeouts/timeout-example.yaml' %}
```

Reference the [timeouts][timeouts] API documentation for additional details.

####### Name (optional)

??? example "Experimental Channel since v1.2.0"

    This concept has been part of the Experimental Channel since `v1.2.0`.
    For more information on release channels, refer to our
    [versioning guide](../concepts/versioning.md).

HTTPRoute Rules include an optional `name` field. The applications for the name of a route rule are implementation-specific. It can be used to reference individual route rules by name from other resources, such as in the `sectionName` field of metaresources ([GEP-2648](../geps/gep-2648/index.md#section-names)), in the status stanzas of resources related to the route object, to identify internal configuration objects generated by the implementation from HTTPRoute Rule, etc.

If specified, the value of the name field must comply with the [`SectionName`](https://github.com/kubernetes-sigs/gateway-api/blob/v1.0.0/apis/v1/shared_types.go#L607-L624) type.

The following example specifies the `name` field to identify HTTPRoute Rules used to split traffic between a _read-only_ backend service and a _write-only_ one:

```yaml
{% include 'experimental/http-route-rule-name.yaml' %}
```

####### Backend Protocol

??? example "Experimental Channel since v1.0.0"

    This concept has been part of the Experimental Channel since `v1.0.0`.
    For more information on release channels, refer to our
    [versioning guide](../concepts/versioning.md).

Some implementations may require the [backendRef][backendRef] to be labeled
explicitly in order to route traffic using a certain protocol. For Kubernetes
Service backends this can be done by specifying the [`appProtocol`][appProtocol]
field.

##### Status

Status defines the observed state of HTTPRoute.

###### RouteStatus

RouteStatus defines the observed state that is required across all route types.

####### Parents

Parents define a list of the Gateways (or other parent resources) that are
associated with the HTTPRoute, and the status of the HTTPRoute with respect to
each of these Gateways. When a HTTPRoute adds a reference to a Gateway in
parentRefs, the controller that manages the Gateway should add an entry to this
list when the controller first sees the route and should update the entry as
appropriate when the route is modified.

The following example indicates HTTPRoute "http-example" has been accepted by
Gateway "gw-example" in namespace "gw-example-ns":
```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: http-example
...
status:
  parents:
  - parentRef:
      name: gw-example
      namespace: gw-example-ns
    conditions:
    - type: Accepted
      status: "True"
```

##### Merging
Multiple HTTPRoutes can be attached to a single Gateway resource. Importantly,
only one Route rule may match each request. For more information on how conflict
resolution applies to merging, refer to the [API specification][httprouterule].

[httproute]: ../reference/spec.md#gateway.networking.k8s.io/v1.HTTPRoute
[httprouterule]: ../reference/spec.md#gateway.networking.k8s.io/v1.HTTPRouteRule
[hostname]: ../reference/spec.md#gateway.networking.k8s.io/v1.Hostname
[rfc-3986]: https://tools.ietf.org/html/rfc3986
[matches]: ../reference/spec.md#gateway.networking.k8s.io/v1.HTTPRouteMatch
[filters]: ../reference/spec.md#gateway.networking.k8s.io/v1.HTTPRouteFilter
[backendRef]: ../reference/spec.md#gateway.networking.k8s.io/v1.HTTPBackendRef
[parentRef]: ../reference/spec.md#gateway.networking.k8s.io/v1.ParentRef
[timeouts]: ../reference/spec.md#gateway.networking.k8s.io/v1.HTTPRouteTimeouts
[appProtocol]: https://kubernetes.io/docs/concepts/services-networking/service/#application-protocol
[sectionName]: ../reference/spec.md#gateway.networking.k8s.io/v1.SectionName

#### GRPCRoute

??? success "Standard Channel since v1.1.0"

    The `GRPCRoute` resource is GA and has been part of the Standard Channel since
    `v1.1.0`. For more information on release channels, refer to our [versioning
    guide](../concepts/versioning.md).

[GRPCRoute][grpcroute] is a Gateway API type for specifying routing behavior
of gRPC requests from a Gateway listener to an API object, i.e. Service.

##### Background

While it is possible to route gRPC with `HTTPRoutes` or via custom, out-of-tree
CRDs, in the long run, this leads to a fragmented ecosystem.

gRPC is a [popular RPC framework adopted widely across the industry](https://grpc.io/about/#whos-using-grpc-and-why).
The protocol is used pervasively within the Kubernetes project itself as the basis for
many interfaces, including:

- [the CSI](https://github.com/container-storage-interface/spec/blob/5b0d4540158a260cb3347ef1c87ede8600afb9bf/spec.md),
- [the CRI](https://github.com/kubernetes/cri-api/blob/49fe8b135f4556ea603b1b49470f8365b62f808e/README.md),
- [the device plugin framework](https://kubernetes.io/docs/concepts/extend-kubernetes/compute-storage-net/device-plugins/)

Given gRPC's importance in the application-layer networking space and to
the Kubernetes project in particular, the determination was made not to allow
the ecosystem to fragment unnecessarily.

###### Encapsulated Network Protocols

In general, when it is possible to route an encapsulated protocol at a lower
level, it is acceptable to introduce a route resource at the higher layer when
the following criteria are met:

- Users of the encapsulated protocol would miss out on significant conventional features from their ecosystem if forced to route at a lower layer.
- Users of the encapsulated protocol would experience a degraded user experience if forced to route at a lower layer.
- The encapsulated protocol has a significant user base, particularly in the Kubernetes community.

gRPC meets all of these criteria, so the decision was made to include `GRPCRoute`in Gateway API.

###### Cross Serving

Implementations that support GRPCRoute must enforce uniqueness of
hostnames between `GRPCRoute`s and `HTTPRoute`s. If a route (A) of type `HTTPRoute` or
`GRPCRoute` is attached to a Listener and that listener already has another Route (B) of
the other type attached and the intersection of the hostnames of A and B is
non-empty, then the implementation must reject Route A. That is, the
implementation must raise an 'Accepted' condition with a status of 'False' in
the corresponding RouteParentStatus.

In general, it is recommended that separate hostnames be used for gRPC and
non-gRPC HTTP traffic. This aligns with standard practice in the gRPC community.
If however, it is a necessity to serve HTTP and gRPC on the same hostname with
the only differentiator being URI, the user should use `HTTPRoute` resources for
both gRPC and HTTP. This will come at the cost of the improved UX of the
`GRPCRoute` resource.

##### Spec

The specification of a GRPCRoute consists of:

- [ParentRefs][parentRef]- Define which Gateways this Route wants to be attached
  to.
- [Hostnames][hostname] (optional)- Define a list of hostnames to use for
  matching the Host header of gRPC requests.
- [Rules][grpcrouterule]- Define a list of rules to perform actions against
  matching gRPC requests. Each rule consists of [matches][matches],
  [filters][filters] (optional), [backendRefs][backendRef] (optional), and
  [name][name] (optional) fields.

<!--- Editable SVG available at site-src/images/grpcroute-basic-example.svg -->
The following illustrates a GRPCRoute that sends all traffic to one Service:
![grpcroute-basic-example](../images/grpcroute-basic-example.png)

###### Attaching to Gateways

Each Route includes a way to reference the parent resources it wants to attach
to. In most cases, that's going to be Gateways, but there is some flexibility
here for implementations to support other types of parent resources.

The following example shows how a Route would attach to the `acme-lb` Gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: grpcroute-example
spec:
  parentRefs:
  - name: acme-lb
```

Note that the target Gateway needs to allow GRPCRoutes from the route's
namespace to be attached for the attachment to be successful.

###### Hostnames

Hostnames define a list of hostnames to match against the Host header of the
gRPC request. When a match occurs, the GRPCRoute is selected to perform request
routing based on rules and filters (optional). A hostname is the fully qualified
domain name of a network host, as defined by [RFC 3986][rfc-3986]. Note the
following deviations from the "host" part of the URI as defined in the RFC:

- IPs are not allowed.
- The : delimiter is not respected because ports are not allowed.

Incoming requests are matched against hostnames before the GRPCRoute rules are
evaluated. If no hostname is specified, traffic is routed based on GRPCRoute
rules and filters (optional).

The following example defines hostname "my.example.com":

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: grpcroute-example
spec:
  hostnames:
  - my.example.com
```

###### Rules

Rules define semantics for matching an gRPC requests based on conditions,
optionally executing additional processing steps, and optionally forwarding
the request to an API object.

####### Matches

Matches define conditions used for matching an gRPC requests. Each match is
independent, i.e. this rule will be matched if any single match is satisfied.

Take the following matches configuration as an example:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
...
matches:
  - method:
      service: com.example.User
      method: Login
    headers:
    - name: version
      value: "2"
  - method:
      service: com.example.v2.User
      method: Login
```

For a request to match against this rule, it must satisfy EITHER of the
following conditions:

 - The `com.example.User.Login` method **AND** contains the header "version: 2"
 - The `com.example.v2.User.Login` method.

If no matches are specified, the default is to match every gRPC request.

####### Filters (optional)

Filters define processing steps that must be completed during the request or
response lifecycle. Filters act as an extension point to express additional
processing that may be performed in Gateway implementations. Some examples
include request or response modification, implementing authentication
strategies, rate-limiting, and traffic shaping.

The following example adds header "my-header: foo" to gRPC requests with Host
header "my.filter.com". Note that GRPCRoute uses HTTPRoute filters for features
with functionality identical to HTTPRoute, such as this.

```yaml
{% include 'standard/grpc-filter.yaml' %}
```

API conformance is defined based on the filter type. The effects of ordering
multiple behaviors are currently unspecified. This may change in the future
based on feedback during the alpha stage.

Conformance levels are defined by the filter type:

 - All "core" filters MUST be supported by implementations supporting GRPCRoute.
 - Implementers are encouraged to support "extended" filters.
 - "Implementation-specific" filters have no API guarantees across implementations.

Specifying a core filter multiple times has unspecified or custom conformance.

If an implementation cannot support a combinations of filters, they must clearly
document that limitation. In cases where incompatible or unsupported
filters are specified and cause the `Accepted` condition to be set to status
`False`, implementations may use the `IncompatibleFilters` reason to specify
this configuration error.

####### BackendRefs (optional)

BackendRefs defines the API objects to which matching requests should be sent. If
unspecified, the rule performs no forwarding. If unspecified and no filters
are specified that would result in a response being sent, an `UNIMPLEMENTED` error code
is returned.

The following example forwards gRPC requests for the method `User.Login` to service
"my-service1" on port `50051` and gRPC requests for the method `Things.DoThing` with
header `magic: foo` to service "my-service2" on port `50051`:

```yaml
{% include 'standard/basic-grpc.yaml' %}
```

The following example uses the `weight` field to forward 90% of gRPC requests to
`foo.example.com` to the "foo-v1" Service and the other 10% to the "foo-v2"
Service:

```yaml
{% include 'standard/traffic-splitting/grpc-traffic-split-2.yaml' %}
```

Reference the [backendRef][backendRef] API documentation for additional details
on `weight` and other fields.

####### Name (optional)

??? example "Experimental Channel since v1.2.0"

    This concept has been part of the Experimental Channel since `v1.2.0`.
    For more information on release channels, refer to our
    [versioning guide](../concepts/versioning.md).

GRPCRoute Rules include an optional `name` field. The applications for the name of a route rule are implementation-specific. It can be used to reference individual route rules by name from other resources, such as in the `sectionName` field of metaresources ([GEP-2648](../geps/gep-2648/index.md#section-names)), in the status stanzas of resources related to the route object, to identify internal configuration objects generated by the implementation from GRPCRoute Rule, etc.

If specified, the value of the name field must comply with the [`SectionName`](https://github.com/kubernetes-sigs/gateway-api/blob/v1.0.0/apis/v1/shared_types.go#L607-L624) type.

##### Status

Status defines the observed state of the GRPCRoute.

###### RouteStatus

RouteStatus defines the observed state that is required across all route types.

####### Parents

Parents define a list of the Gateways (or other parent resources) that are
associated with the GRPCRoute, and the status of the GRPCRoute with respect to
each of these Gateways. When a GRPCRoute adds a reference to a Gateway in
parentRefs, the controller that manages the Gateway should add an entry to this
list when the controller first sees the route and should update the entry as
appropriate when the route is modified.

##### Examples

The following example indicates GRPCRoute "grpc-example" has been accepted by
Gateway "gw-example" in namespace "gw-example-ns":

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: grpc-example
...
status:
  parents:
  - parentRefs:
      name: gw-example
      namespace: gw-example-ns
    conditions:
    - type: Accepted
      status: "True"
```

##### Merging
Multiple GRPCRoutes can be attached to a single Gateway resource. Importantly,
only one Route rule may match each request. For more information on how conflict
resolution applies to merging, refer to the [API specification][grpcrouterule].

[grpcroute]: ../reference/spec.md#gateway.networking.k8s.io/v1.GRPCRoute
[grpcrouterule]: ../reference/spec.md#gateway.networking.k8s.io/v1.GRPCRouteRule
[hostname]: ../reference/spec.md#gateway.networking.k8s.io/v1.Hostname
[rfc-3986]: https://tools.ietf.org/html/rfc3986
[matches]: ../reference/spec.md#gateway.networking.k8s.io/v1.GRPCRouteMatch
[filters]: ../reference/spec.md#gateway.networking.k8s.io/v1.GRPCRouteFilter
[backendRef]: ../reference/spec.md#gateway.networking.k8s.io/v1.GRPCBackendRef
[parentRef]: ../reference/spec.md#gateway.networking.k8s.io/v1.ParentRef
[name]: ../reference/spec.md#gateway.networking.k8s.io/v1.SectionName

#### TCPRoute
*[To be populated with complete TCPRoute API specification]*

#### TLSRoute
*[To be populated with complete TLSRoute API specification]*

#### UDPRoute
*[To be populated with complete UDPRoute API specification]*

#### ReferenceGrant

??? success "Standard Channel since v0.6.0"

    The `ReferenceGrant` resource is Beta and part of the
    Standard Channel since `v0.6.0`. For more information on release
    channels, refer to our [versioning guide](../concepts/versioning.md).

!!! note
    This resource was originally named "ReferencePolicy". It was renamed
    to "ReferenceGrant" to avoid any confusion with policy attachment.

A ReferenceGrant can be used to enable cross namespace references within
Gateway API. In particular, Routes may forward traffic to backends in other
namespaces, or Gateways may refer to Secrets in another namespace.

![Reference Grant](../images/referencegrant-simple.svg)
<!-- Source: https://docs.google.com/presentation/d/11HEYCgFi-aya7FS91JvAfllHiIlvfgcp7qpi_Azjk4E/edit#slide=id.g13c18e3a7ab_0_171 -->

In the past, we've seen that forwarding traffic across namespace boundaries is a
desired feature, but without a safeguard like ReferenceGrant,
[vulnerabilities](https://github.com/kubernetes/kubernetes/issues/103675) can
emerge.

If an object is referred to from outside its namespace, the object's owner must
create a ReferenceGrant resource to explicitly allow that reference. Without a
ReferenceGrant, a cross namespace reference is invalid.

##### Structure
Fundamentally a ReferenceGrant is made up of two lists, a list of resources
references may come from, and a list of resources that may be referenced.

The `from` list allows you to specify the group, kind, and namespace of
resources that may reference items described in the `to` list.

The `to` list allows you to specify the group and kind of resources that may be
referenced by items described in the `from` list. The namespace is not necessary
in the `to` list because a ReferenceGrant can only be used to allow references
to resources in the same namespace as the ReferenceGrant.

##### Example
The following example shows how a HTTPRoute in namespace `foo` can reference a
Service in namespace `bar`. In this example a ReferenceGrant in the `bar`
namespace explicitly allows references to Services from HTTPRoutes in the `foo`
namespace.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: foo
  namespace: foo
spec:
  rules:
  - matches:
    - path: /bar
    backendRefs:
      - name: bar
        namespace: bar
---
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: bar
  namespace: bar
spec:
  from:
  - group: gateway.networking.k8s.io
    kind: HTTPRoute
    namespace: foo
  to:
  - group: ""
    kind: Service
```

##### API design decisions
While the API is simplistic in nature, it comes with a few notable decisions:

1. Each ReferenceGrant only supports a single From and To section. Additional
   trust relationships must be modeled with additional ReferenceGrant
   resources.
1. Resource names are intentionally excluded from the "From" section of
   ReferenceGrant because they rarely provide any meaningful protection. A user
   that is able to write to resources of a certain kind within a namespace can
   always rename resources or change the structure of the resources to match a
   given grant.
1. A single Namespace is allowed per "From" struct. Although a selector would be
   more powerful, it encourages unnecessarily insecure configuration.
1. The effect of these resources is purely additive, they stack on top of each
   other. This makes it impossible for them to conflict with each other.

Please see the [API
Specification](../reference/spec.md#gateway.networking.k8s.io/v1alpha2.ReferenceGrant)
for more details on how specific ReferenceGrant fields are interpreted.

##### Implementation Guidelines
This API relies on runtime verification. Implementations MUST watch for changes
to these resources and recalculate the validity of cross-namespace references
after each change or deletion.

When communicating the status of a cross-namespace reference, implementations
MUST NOT expose information about the existence of a resource in another
namespace unless a ReferenceGrant exists allowing the reference to occur. This
means that if a cross-namespace reference is made without a ReferenceGrant to a
resource that doesn't exist, any status conditions or warning messages need to
focus on the fact that a ReferenceGrant does not exist to allow this reference.
No hints should be provided about whether or not the referenced resource exists.

##### Exceptions
Cross namespace Route -> Gateway binding follows a slightly different pattern
where the handshake mechanism is built into the Gateway resource. For more
information on that approach, refer to the relevant [Security Model
documentation](../concepts/security-model.md). Although conceptually similar to
ReferenceGrant, this configuration is built directly into Gateway Listeners,
and allows for fine-grained per Listener configuration that would not be
possible with ReferenceGrant.

There are some situations where it MAY be acceptable to ignore ReferenceGrant
in favor of some other security mechanism. This MAY only be done if other
mechanisms like NetworkPolicy can effectively limit cross-namespace references
by the implementation.

An implementation choosing to make this exception MUST clearly document that
ReferenceGrant is not honored by their implementations and detail which
alternative safeguards are available. Note that this is unlikely to apply to
ingress implementations of the API and will not apply to all mesh
implementations.

For an example of the risks involved in cross-namespace references, refer to
[CVE-2021-25740](https://github.com/kubernetes/kubernetes/issues/103675).
Implementations of this API need to be very careful to avoid confused deputy
attacks. ReferenceGrant provides a safeguard for that. Exceptions MUST only be
made by implementations that are absolutely certain that other equally effective
safeguards are in place.

##### Conformance Level
ReferenceGrant support is a "CORE" conformance level requirement for
cross-namespace references that originate from the following objects:

- Gateway
- GRPCRoute
- HTTPRoute
- TLSRoute
- TCPRoute
- UDPRoute

That is, all implementations MUST use this flow for any cross namespace
references in the Gateway and any of the core xRoute types, except as noted
in the Exceptions section above.

Other "ImplementationSpecific" objects and references MUST also use this flow
for cross-namespace references, except as noted in the Exceptions section above.

##### Potential Future API Group Change

ReferenceGrant is starting to gain interest outside of Gateway API and SIG
Network use cases. It is possible that this resource may move to a more neutral
home. Users of the ReferenceGrant API may be required to transition to a
different API Group (instead of `gateway.networking.k8s.io`) at some point in
the future.

## Conformance Requirements

*[To be populated with complete conformance requirements and test profiles]*

## Feature Support Matrix

*[To be populated with standard vs extended feature classifications]*

---

*Reference documentation to be populated with official Kubernetes Gateway API specification content.*
