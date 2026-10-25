---
"briardr": minor
---

**Releases come with archives that run Briardr without a checkout.** Each GitHub release now has
`briardr-<os>-<arch>.tar.gz` for macOS and Linux on aarch64 and x86_64, with a `SHA256SUMS` file: the
server as `bin/briardr`, the web client next to it, and the third-party license notices. Unpack it and run
`bin/briardr`; no Git checkout, Bun or Rust needed. The Linux archives need glibc 2.39 or newer (Ubuntu
24.04 and later). Making and building plugins still needs a checkout of the same version.
