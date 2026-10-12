---
name: crossmatrix
description: "Method reference for the crossmatrix-mcp tools (crossmatrix.command and crossmatrix.query): a generic, multidimensional House of Quality / weighted cross-reference matrix. Use when you build, fill or read any crossmatrix model, or when no narrower crossmatrix-* skill fits. Covers the Configuration / Dimensions / State split, observation-only writes (tokens plus evidence, never numbers), the driver loop (gaps.next, observe, analyze), scoped slices, optimistic versions and requestIds, links, and the committed .crossmatrix/ store."
---

# crossmatrix

crossmatrix keeps a sparse, weighted cross-reference between **dimensions** (lists of
things: needs, characteristics, jobs, tests, risks, deliverables...). You make the
judgments; the engine does every number. Domain meaning lives in your configuration and
in these skills, never in the engine.

## Three documents, linked by id@version

| Layer | Holds | Op family |
|---|---|---|
| Configuration | scales, relation declarations, contractions | `scale.declare`, `relation.declare`, `contraction.declare` |
| Dimensions | dimensions and their members (rosters, weights) | `dimension.register`, `members.sync`, `member.propose` |
| State | cells: observations on member pairs | `observe`, `evidence.attach`, `deprecate` |

Findings are computed on every read and are never stored.

## The store: `.crossmatrix/` (commit it)

Run `crossmatrix-mcp init` once at the repo root. It creates `.crossmatrix/` with
`config/<configId>@<version>.json`, `dimensions/<dimensionSetId>@<version>.json`,
`state/<stateId>.json`, an append-only `audit/<stateId>.jsonl` (union-merged), a README and
a `.gitattributes`. Files are canonical JSON, so diffs are small. **Commit the folder.** Run
`crossmatrix-mcp check` after merges and in CI; it exits 1 with path-specific diagnostics.

The server picks its store from `CROSSMATRIX_DIR`, else the nearest `.crossmatrix/` above
its working directory, else a per-user data directory. Confirm `root` in `model.status`
before you write.

## Calling the tools

Every write is one `crossmatrix.command`:

```json
{"request": {"schemaVersion": "0.2.0", "requestId": "obs-0007", "modelId": "checkout",
  "expectedVersion": 6, "actor": {"actorType": "llm", "persona": "observer"},
  "op": {"kind": "observe", "relationId": "rel_need_char", "from": "need_fast",
         "to": "char_latency", "observation": "strong", "provenanceClass": "inferred",
         "evidence": [{"sourceRef": {"sourceRef": "docs/perf.md", "locator": "L10-L24"}}]}}}
```

- `requestId` is an idempotency key: resending it replays the first answer.
- `expectedVersion` is the version you last saw. A `version_conflict` means someone else
  wrote; re-read `model.status` and retry.
- Every response, including a rejection, carries `links`: the legal next ops. A rejection
  carries `error.code`; fix the cause the code names, then follow a link.

Reads are `crossmatrix.query` with `{"request": {"query": {"kind": "...", ...}}}`.

## Observation-only

- A cell value is a **token** from the relation's scale (`none|weak|moderate|strong` or
  whatever the scale declares), never a number.
- Member importance is a `weightObservation` token from the dimension's `weightScale`. A
  numeric `weight` is accepted only from a deterministic source with `sourceRef.system`
  (for example an FMECA criticality import).
- `provenanceClass`: `normative` (a decision), `observed` (measured or seen), `inferred`
  (your judgment; **evidence is required**). A `measuredValue` needs `observed` plus a
  `sourceRef`.
- Support and opposition are separate relations (`supports` vs `opposes`, `mitigates` vs
  `exposes`), so strong-for and strong-against never cancel; the engine reports net and
  tension.

## Build a model

1. `model.open` with `stateId` (creates it if absent; reopens it after a restart).
2. `scale.declare` your relationship scale and an importance scale.
3. `dimension.register` each axis (`dimensionKind` is any snake_case word; set
   `weightScale` on weighted axes), then `members.sync` its roster.
4. `relation.declare` each pair of axes (`from == to` is a correlation roof), with a
   `coveragePolicy` (`every_from`, `every_to`, `both`) when gaps must be filled.
5. `contraction.declare` a chain when you need a derived A→C view through B.

## The driver loop

Do not fill cells in bulk from memory. Let the engine pick:

1. `gaps.next` → the most important uncovered members.
2. Open the evidence, then `observe` one cell (token + rationale + evidence).
3. Repeat until `gaps.next` is empty or the remaining gaps are knowingly `none`.
4. `analyze.findings`, `conflicts`, `coverage`, `analyze.marginalize` for priorities.

## Scoped reads

Never ask for the whole model when a slice answers the question: `describe` (structure),
`slice {relationId, from?, to?}`, `trace {member}`, `explain {relationId, from, to}`,
`coverage`, `gaps.orphans`. Use `export` only to hand the model to another tool, and
`report` for a human-readable House of Quality.

## Large rosters, hierarchies and sources

- Sync rosters straight from their source (files, tickets, tests) with a per-member
  `sourceRef {"uri": "file://src/lib.rs", "hash": "sha256:..."}`. `members.sync` handles
  10k+ members; re-sending an unchanged roster is a no-op (no new version).
- Model nesting (directory contains file, epic contains story) as a `contains`
  self-relation, parent → child. Add `rollup: true` to `trace`, `slice`, `coverage`,
  `gaps.next` or `analyze.*` to roll leaf observations up to ancestors.
- `trace {"member": "<id or uri>"}` and `slice {"relationId", "from"|"to": "<id or uri>"}`
  work in both directions and page with `limit` / `cursor` (`nextCursor`).
- `stale {"hashes": {"<uri>": "<hash>"}}` (or `"readFiles": true` for `file://` sources
  inside the project) returns the changed members and every cell that rests on them.

## Changing your mind

Never delete. `deprecate {targetId, reason, supersededBy?}` retires a member or relation;
deprecated elements leave analyses but stay in history. A re-synced roster deprecates
members that disappeared from the source.

Narrower skills: [crossmatrix-hoq](../crossmatrix-hoq/SKILL.md),
[crossmatrix-jtbd](../crossmatrix-jtbd/SKILL.md),
[crossmatrix-competitive](../crossmatrix-competitive/SKILL.md),
[crossmatrix-test-coverage](../crossmatrix-test-coverage/SKILL.md),
[crossmatrix-risk](../crossmatrix-risk/SKILL.md),
[crossmatrix-prioritize](../crossmatrix-prioritize/SKILL.md),
[crossmatrix-review](../crossmatrix-review/SKILL.md).
