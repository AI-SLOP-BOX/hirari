# Worktree change policy

Large migrations and native-engine experiments can temporarily produce a noisy
worktree. Aura keeps that noise reviewable without deleting or resetting user
work. The canonical classifier is:

```sh
./scripts/classify_worktree_changes.sh artifacts/worktree-classification.tsv
```

The report has four intentional categories:

- `product_source`: Rust, Slint, C++, Objective-C++, shaders, and build scripts
  that affect the shipped product.
- `documentation`: README, policies, release notes, and design records.
- `tooling`: CI, audit, setup, and test harnesses.
- `generated`: build products, logs, packaged apps, caches, and temporary audit
  output. These are evidence artifacts, not source changes.

The `aura-core-bridge` nested repository is recorded as one boundary entry so
its own history can be reviewed independently. The classifier never removes,
stages, resets, or rewrites files. Reviewers should inspect source and tooling
first, then generated evidence, and finally compare the nested repository at
its recorded revision.

Generated evidence belongs under `artifacts/` or `release-metadata/`; both are
ignored for normal commits. `Cargo.lock` is intentionally tracked because Aura
is an application and reproducible dependency resolution is part of its build
contract.
