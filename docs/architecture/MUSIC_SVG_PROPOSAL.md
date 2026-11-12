# Architecture Decision Record: "Textual Patch" program (from the friend proposal)

**Status**: APPROVED — pursued seriously. This is now a program of record, not a maybe.
**Date**: 2026-11-12 (revised from an earlier, weaker "deferred" version — the maintainer
decided with fresh eyes that this idea deserves a real engineering effort).

A friend proposed a specification-adjacent idea: an editable text format
with declarative dataflow devices, opaque plugin binaries, unified curve
representation, and lock-free hot-swapping. An earlier review judged parts
of it by its packaging instead of its substance. The substance is good.
This ADR records the commitment and the honest scope of "serious".

## Naming and ambition, stated honestly

Two things must not happen:

1. **Do not brand it "music SVG" or any other already-claimed standard
   word.** It is a private format. The ambition is not to lie about
   approval bodies; it is to be *engineered as if* a standards body will
   show up: written spec, versioned, conformance-tested, deterministic.
2. **Do not pretend the gap list is solved.** The claim "new standard"
   is aspirational until there is a spec document, conformance vectors,
   and at least one reader other than us. Everything else is vibes.

So: the program aims at **specification-grade engineering inside a
private text format**, not at external recognition. If the quality bar is
met, the naming question answers itself later.

Working name: **textual patch**.

## Verdict table

| Proposal component | Status in Hirari (evidence) | Verdict |
| --- | --- | --- |
| Unify curves (one model, three consumers) | automation stores `AutomationPointContract{time, value, curve}` and FFIs as `NativeAutomationPoint`; UI paints its own copy; save is separate serialization | **Adopt (Phase A)** |
| `BuiltinDataflow` declarative devices | `PluginInstanceContract` + checksummed base64 state; DawProject roundtrip exists | **Adopt (Phase B)** |
| Deterministic text project format | native pretty JSON contracts, atomic save, `diff_project_json` compat | **Keep and harden (Phase C)** |
| Specification-grade engineering | no public written spec; reader ecosystem = zero | **Build it (Phase C)** |
| arc-swap / lock-free RT graph | `arc-swap` used for tempo/panner/effect-chain snapshots already | **Keep as-is; measure before expanding** |
| KDL/XML as first-class format | two formats already in tree; KDL is aesthetic preference without added correctness | **Rejected as canonical; optional export only** |
| ARA2 "fits perfectly" | `ara2_protocol.rs` is a contract layer; no Celemony SDK bound | **Rejected until SDK licensing exists** |
| "Music SVG" naming | misleading (nothing is SVG; it is JSON) | **Rejected** |

## Program roadmap

The 800-line split campaign continues to gate this work. Do not start
Phase A/B opportunistically inside the split campaign; both touch the RT
path and the schema respectively.

- **Phase 0 — split campaign** (in progress).
- **Phase A — curve unification** (first after the split):
  - New contract type `Curve::{Linear, Step, Bezier { p: [Point; 2] } }`
    (keep `Shortcut { curve: f32 }` for backward compat).
  - One evaluator used by UI scene build, RT path, and serialization.
    RT keeps hint-cached scanning in `realtime_automation.rs`.
  - Save upgrades: older projects load cleanly, store the new form; the
    writer never emits mixed representations for the same entity.
  - Acceptance: equivalence tests between the UI-lane evaluator and the
    FFI-lane evaluator across randomized point sets; a pre-split project
    round-trips its automation byte-identically.
- **Phase B — `BuiltinDataflow` devices**:
  - `PluginInstanceContract` grows `kind: BuiltinDataflow { units, links }`.
  - External plugins stay opaque (base64 + checksum). The AI edit boundary
    is *formally enforced*: serializers reject unknown keys in the
    `BuiltinDataflow` branch shape at load; AI tooling documents what it
    may edit (only that branch).
  - DawProject passthrough extracts/writes the same units as custom device
    or opaque — see Phase C mapping.
  - Acceptance: roundtrip through save/load + DawProject XML leaves plugin
    binaries provably untouched (hash comparison).
- **Phase C — specification release**:
  - New doc `docs/architecture/TEXTUAL_PATCH.md` versioned per schema.
    Contains: purpose, notation (JSON profile), canonical ordering,
    determinism guarantees, versioning policy (minor additions ignore
    unknown fields; major bumps reject older readers), required/optional
    field tables, conformance test vectors (parse + emit + roundtrip),
    reference implementation pointer, DawProject mapping table, AI editing
    boundary with allowed/blocked example diffs.
  - Conformance tests live in the contract test suite; release gate wants
    these green before any public announcement.
  - Steps before external publication: two correct readers agree on every
    conformance vector; we do not publish a specification we cannot
    conform to.
- **Phase D — optional distribution**:
  - `.hirpatch` sidecar for device-only sharing, with a strict importer
    that validates all units/links/limits.
  - Nice-to-have, not a gate; keep optional forever.

## What "serious" means in practice (guardrails)

- No spec document, no claims. Tests before phase names.
- Deterministic output is non-negotiable: pretty-printer key order, array
  order, escape rules, numeric formatting — all pinned by golden tests.
- Every claim in the spec has a test in the contract suite.
- No external dependencies other than serde-compatible JSON; avoiding
   KDL/XML keeps two readers in sync, not three.
- If and when this ships externally, it ships with the conformance suite
  and the DawProject mapping — not with hype.

## Explicitly rejected, with reasons

1. **KDL/XML as canonical.** Same correctness shape as JSON; doubles the
   reader maintenance surface; loses the tooling we already have.
2. **SVG/visual language rebrand.** Brings false expectations (nothing is
   vector graphics; it is time-domain dataflow and audio automation).
3. **ARA2 wiring now.** No Celemony SDK is bound. Anything called ARA2
   before licensing is papering over a gap. We re-evaluate after Phase B.
4. **Full-lock-free RT graph rewrite.** The existing effect-chain
   snapshot machinery is already the correct pattern. Blind rewrites of
   the RT path are for architecture astronautics, not DAW shipping.

## Process notes

- Verify with Linux CI (`cargo check --workspace --locked`,
  `cargo test --workspace --locked`) — local builds are disk-constrained;
  treating CI as the source of truth.
- `ame-ui` is a path dep; do not commit a `Cargo.lock` generated against
  a dirty `../AmeUI` working tree.
- Automation RT evaluation must remain `repr(C)`-compatible at the FFI;
  do not make the FFI strategy fancier before Phase A's tests exist.

## Sequencing decision

1. 800-line split campaign finishes (in progress).
2. Phase A spike → land behind an explicit test gate.
3. Phase B after A (schema additive, separate commit).
4. Phase C spec lands once Phases A+B are green: docs + conformance suite
   in the same PR series; docs live in-repo and stay versioned.
5. Re-evaluate ARA2 only after Phase B and SDK availability.

This document is the commitment. Do not start Phases A/B mid-flight in the
split campaign. When the split finishes, open the first spike for Phase A
and link it here.
