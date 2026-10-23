# 13. A tag's annotation can carry hints that skip work, never identity

Status: Proposed

## Context

- ADR 9 makes a tag's identity its commit and being annotated, and does not compare annotation text: the remote check (`git ls-remote`) cannot read it, and a hand-made or older tag would otherwise block every rerun. It names its revisit trigger: something downstream starts reading the annotation text.
- Two reads dominate a workspace of thousands of packages. Confirming that a bare-named tag was made by this package reads its manifest at the tag (about 300 ms at 10,000 packages). Deciding whether a package changed since its tag compares each of its directories' tree ids at the tag with `HEAD` (about 265 ms), which needs the tag's commit trees.
- Callisto knows both facts when it creates the tag: who the package is and what each of its directories held in the release commit.
- A shallow or partial checkout has to fetch those commits and trees, or cannot answer.

## Decision

- The annotation Callisto writes keeps `Release {tag}` as its first line and adds git trailers: `Callisto-Hints: 1`, `Callisto-Package: <canonical ref>` and one `Callisto-Tree: <directory> <tree id>` per package directory (`.` for a root package). The trailers are read by the same `git for-each-ref` that lists the tags.
- A hint only lets a read be skipped. It never makes a tag adopted and never decides identity; ADR 9 is unchanged. Release effects are still derived from commits and the committed decision (ADR 2).
- Every consumer works without a hint. A hint is used only when the tag is annotated, `Callisto-Hints` is a version this release understands, the trailer it needs parses, and, for the owner, it names this package's canonical ref or, for the trees, it has an entry for every directory of the package. In every other case, including a hint that names another package, the consumer does the read it does today and that read decides.
- Existing tags are never rewritten. The fallback is permanent, not a migration step.

## Options considered

- **A ref namespace holding an index** (`refs/callisto/...`): rejected. It is extra refs to fetch and push, it can go stale against the tags, and it needs its own trust story. The tag is already the per-release record and travels with it.
- **A persistent local cache keyed by commit**: rejected for this purpose. It helps warm local runs only, and CI starts cold.
- **Skip the owner check when the name is unique among current packages**: rejected. It loses the protection against a tag made by a package that has since been renamed or removed, which is the case the check exists for.
- **Compare annotation text for identity**: rejected for the reasons in ADR 9.
- **A git note**: rejected. Notes are not fetched by default and are not part of the tag object.

## Consequences

- At 10,000 packages the two reads are about 0.55 s of a 2.2 s command; at 400 packages they are tens of milliseconds. The larger value is a shallow or partial checkout that no longer has to read the tag commit's trees or manifest.
- The annotation of new tags is longer. It is part of the prepared tag operation and not of the sealed intent, so no intent digest changes.
- A tag object is immutable, so a hint cannot go stale. It can be wrong if someone writes a false annotation, which is the trust ADR 9 already extends to any annotated tag on the planned commit; a wrong hint changes what a read reports, never what an effect does.
- Hints and the fallback are two code paths per consumer, each tested against the same repository.

## Enforcement

- For each consumer, tests for no hint at all (a lightweight tag, an annotation that is only `Release {tag}`, an unknown hint version, a missing, garbled or partial trailer, a hint naming another package, a directory with no entry) that assert the same result as today.
- An equivalence test that builds one repository with and without hints and asserts the same `status` and tag index.
- Spec criteria in `docs/specs/release.json` and `docs/specs/cli.json` stating that a hint never changes a result.

## Revisit when

- A consumer needs a hint to be authoritative.
- Tags become signed or carry provenance that reads the annotation differently.
- A measured saving at the sizes in use is too small to justify the second path.

## Sources

- ADR 9, its revisit trigger.
- PR #330 and PR #333: the profiles and timings at 400 to 10,000 packages.
