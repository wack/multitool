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
