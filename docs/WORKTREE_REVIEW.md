# Worktree review guide

The repository currently contains a large migration-scale worktree. To keep
review independent from generated build output, classify changes before each
review:

```sh
./scripts/classify_worktree_changes.sh
```

The classifier emits a TSV path manifest and counts four categories:

| Category | Meaning | Review treatment |
| --- | --- | --- |
| `product_source` | Aura engine, UI, project model, tests, and owned resources | Review for behavior and API changes |
| `tooling` | Build, packaging, CI, audit, and developer scripts | Review for reproducibility and release safety |
| `documentation` | User, contributor, security, and release documentation | Review for accuracy and completeness |
| `generated` | Bundles, logs, build products, and other derived files | Do not include in source commits |

The classifier is intentionally read-only. It does not stage, delete, or reset
anything. The current snapshot (2026-08-29) reports 1,565 product-source,
53 tooling, 51 documentation, and 175 generated entries. Existing deletions
are preserved and shown with their Git status so reviewers can distinguish
intentional removals from modified files.

Before submitting a change, run the following gates from a clean build
environment:

```sh
./scripts/audit_repository_hygiene.sh
./scripts/verify_repository_health.sh
cargo test --workspace --all-targets --locked -- --test-threads=1
```
