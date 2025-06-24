# Running Conformance Tests

1. Clone the Gateway API repo:

```bash
$ git clone git@github.com:kubernetes-sigs/gateway-api.git
```

1. Make sure you're kubectl context is pointed at dev:

```bash
$ kubectl config current-context
do-nyc3-multitool-development
```

1. Load the Gateway Class CRD into the cluster:

```bash
$ cargo make load-gatewayclass
```

1. Run the conformance tests:

```bash
$ go test ./conformance -run TestConformance -args \
          --gateway-class=multitool-gateway-class \
          --supported-features=Gateway
```

1. When you're done, remember to unload the CRD:

```bash
$ cargo make unload-gatewayclass
```

# Contributing

To generate the CRD code again, install `kopium` and run:

```bash
$ cargo make gen-crds
```

The CRD definitions themselves are kept at `.crds/`, which is stored at the
top level of this repo (__not__ the top level of this crate).

To update the version of the CRDs used in this repo, you can download
a particular release from GitHub. Then, you must split each
of the YAML documents into their own file. NB: We should probably
write a one-liner script to do that automatically.
