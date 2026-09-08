# 04 — Risk Register

Directive §31–35 and §79 ask for risks to be named, including ones not in the
specification. Severity is the product of likelihood and consequence for the
*vision*, not for any single sprint.

---

## Critical

### R1 — The reactive cascade does not scale, and it is the product's core claim
**Measured.** One dialogue edit: 2.15 ms at 7 scenes → 144 ms at 1,000. The
differentiator of CINE-OMNI is cross-domain propagation; if propagation cannot
stay interactive at feature scale, the differentiator becomes the bottleneck.
→ *Mitigation:* Phase 1.3, with a committed benchmark as the acceptance gate.

### R2 — No media engine exists, and five phases depend on it
No decode, no encode, no image reading. Editorial, VFX, color, audio and dailies
are all blocked behind it. It is also the largest single body of integration
work and carries the licensing and patent exposure.
→ *Mitigation:* Phase 2 is treated as the critical path, not as one phase among
many.

### R3 — Scope versus sustainable delivery
The vision spans twenty disciplines that each support entire companies. The risk
is not that the architecture is wrong but that the surface area outruns the
capacity to maintain it, and the product becomes many shallow imitations —
precisely what §30 and §73 forbid.
→ *Mitigation:* depth-first per phase with honest capability labelling; never
mark a subsystem complete against §59's gate list. Prefer a small number of
genuinely professional workspaces over twenty shallow ones.

---

## High

### R4 — The application shell cannot deliver the product's own UI requirements
A browser tab over a WebSocket cannot do docking, multi-monitor, native GPU
viewport surfaces, or §39's local-first requirement. It also spawns one engine
per tab, so two tabs diverge. Deferring this decision means UI work done now may
be discarded.
→ *Mitigation:* the shell research is the highest-value blocked item; do not
build significant UI until it lands.

### R5 — Claiming standards compliance that has not been earned
The product already touches ACES, OTIO, USD and OCIO by name while implementing
approximations or subsets. §60 forbids the claim; the commercial temptation to
make it is real, and a single overclaim damages credibility with the
professional audience the product is for.
→ *Mitigation:* the honesty rules are written into the blueprint and
`CHATGPT_CONTEXT.md`; conformance claims require a passing conformance test.

### R6 — Codec patent exposure, distinct from copyright
Copyright licensing is analysed; **patent and royalty exposure is not**. H.264
and H.265 encoding in particular carry pool licensing obligations independent of
FFmpeg's LGPL terms.
→ *Mitigation:* analyse before Phase 2 ships; prefer hardware encoders and
royalty-free codecs where possible.

### R7 — The UCG becomes a bulk data store
At ~2 KB/node, "millions of records" is ~2 GB before media. Per-frame and time
series data must not be graph nodes.
→ *Mitigation:* Phase 1.6 draws the boundary before the data arrives.

---

## Medium

### R8 — Single global write lock
Correct today, will not survive twenty workspaces plus background workers plus
collaboration. Gated behind Phase 1.4.

### R9 — Collaboration may need a CRDT the current model cannot become
The op log carries `(actor, lamport)`, which is a good start but is not a
conflict-resolution strategy. If a real CRDT is required, retrofitting it after
twenty domains have shipped would be extremely expensive.
→ *Mitigation:* resolve during the blocked production research, before Phase 9.

### R10 — GPL boundary erosion
The rule is clear now, but under delivery pressure "just look at how Blender
does it" becomes "paste it". A TYPE G reimplementation must be defensibly
independent.
→ *Mitigation:* written record for each TYPE G decision; `cargo-deny` in CI.

### R11 — Research completed under rate limiting may be uneven
Two of seven clusters landed; one of those stopped before covering OpenUSD. Any
matrix built on partial research inherits its gaps.
→ *Mitigation:* gaps are marked `PENDING`/`UNVERIFIED` rather than filled;
matrices are explicitly blocked rather than published half-evidenced.

---

## Risks the directive did not name

### R12 — There was no version control
~50,000 lines of validated work with no recovery path. **Resolved** at the start
of this engagement: commit `7a82b41`. Recording it because it was the single
largest unmanaged risk and would have made any re-architecture reckless.

### R13 — Benchmarks decay without enforcement
The scale benchmarks are `#[ignore]`d, so CI will not catch a regression that
reintroduces the 74× defect.
→ *Mitigation:* a performance gate in CI once Phase 1 establishes the targets.

### R14 — The demo is unrepresentative and may mislead planning
`cine demo` runs 7 scenes and settles in 2 ms, which looks excellent and hides
the scaling defect entirely. Anyone judging readiness from the demo will
overestimate it.
→ *Mitigation:* the archaeology numbers are published alongside; consider making
the demo default to feature scale once Phase 1.3 lands.

### R15 — Single-machine, single-run measurements
All figures come from one 4-core machine with an Intel integrated GPU, run once.
They establish scaling *shape* reliably and absolute constants only loosely.
→ *Mitigation:* re-measure on target-class hardware before setting performance
acceptance thresholds.
