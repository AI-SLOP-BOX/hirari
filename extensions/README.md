# Aura extensions

This directory contains manifest-only example extensions.  Aura discovers
`<extension-id>/manifest.json` entries without executing extension code.

An extension command must declare a stable ID, a user-facing title, and a
mutation class (`read_only` or `reversible`).  Extensions default to
`execution: "sandboxed"`.  A power-user extension may declare
`execution: "trusted"` and request external filesystem, network, or process
permissions, but Aura must explicitly approve that extension before activation
and record the decision in its audit log.  A manifest never grants those
permissions by itself.

The included `aura-mix-inspector` is intentionally read-only.  It is the
reference shape for a future panel or CLI command that reports loudness,
headroom, clipping, phase, and sandbox health.
