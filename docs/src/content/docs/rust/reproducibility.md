---
title: "Reproducibility by design"
description: "How the Bismark Rust suite makes its results reproducible: byte-identity to Perl v0.25.1, pinned tool versions, reproducible builds, per-output provenance sidecars and in-BAM provenance, and a one-command smoke test."
---

A result is only reusable if you can tell exactly how it was produced: which data, which tool,
which version, which parameters. The Bismark Rust suite is built so that answer is always
recoverable, without any extra bookkeeping on your part. This page gathers the machinery.

## 1. Byte-identity to Perl v0.25.1

Every faithful tool path reproduces Perl Bismark `v0.25.1` output byte-for-byte: not just the
numbers, but field order, rounding, tie-breaking, header lines and report text. This is a CI gate,
not an aspiration. Validation is per-tool golden fixtures plus live-Perl oracles that run the
in-repo Perl v0.25.1 scripts and diff their output against the Rust output on every pull request
(the `perl-oracle` job, which also proves none of the oracles silently skipped).

Anything that cannot be byte-identical (a new algorithm, a faster index layout) is an opt-in,
never-silent, concordance-gated flag, leaving the default path frozen.

## 2. Pinned environment

The published container image pins every external dependency to the exact version the
byte-identity gates were validated against:

| Component | Pinned version |
|---|---|
| Bowtie 2 | 2.5.5 |
| HISAT2 | 2.2.2 |
| minimap2 | 2.31 |
| Rust toolchain (MSRV) | 1.89 |

Rust dependencies are exact-pinned (`=x.y.z`) and the build uses `--locked`, so the dependency
graph cannot drift between builds. No Samtools or htslib is needed: BAM/SAM/CRAM I/O is pure-Rust
via [noodles](https://github.com/zaeleus/noodles).

## 3. Reproducible builds

The build timestamp honours [`SOURCE_DATE_EPOCH`](https://reproducible-builds.org/docs/source-date-epoch/),
so two builds of the same commit under the same epoch produce bit-for-bit identical binaries. This
is verified locally with `just reproduce` (build twice, `cmp` every binary) and, as of the
reproducibility-by-design work, enforced in CI on every pull request by the `reproduce` job.

## 4. Provenance you do not have to ask for

Each binary reports its true build via `--version`:

```
bismark (Bismark Rust suite) v3.0.0 (abc1234 — linux/x86_64 — built 2026-07-11T09:12:00Z)
```

That is the suite version, the git short-hash of the build commit, the target, and a reproducible
build timestamp.

Beyond the banner, every tool that writes a primary output records how it was produced, two
complementary ways, both always on:

### Provenance sidecar

Next to each output file, the suite writes a companion
`<output>.bismark_provenance.json`. It records the schema version, the tool, the suite version, the
git hash, the build and run timestamps, the emulated Perl version, the verbatim command line, the
working directory, the OS and architecture, and each input file's path, size and modification time.
Because it is a brand-new file (Perl never wrote one), it changes zero bytes of the byte-identical
primary output.

```json
{
  "schema_version": 1,
  "tool": "bismark",
  "suite_version": "3.0.0",
  "git_short_hash": "abc1234",
  "build_timestamp": "2026-07-11T09:12:00Z",
  "run_timestamp": "2026-07-11T14:03:11Z",
  "emulated_perl_version": "v0.25.1",
  "command_line": ["bismark", "--genome", "/g", "-1", "r1.fq", "-2", "r2.fq"],
  "working_dir": "/work",
  "os": "linux",
  "arch": "x86_64",
  "primary_output": "sample_bismark_bt2_pe.bam",
  "inputs": [
    {"path": "r1.fq", "size_bytes": 12345, "mtime_epoch_secs": 1700000000}
  ]
}
```

### In-BAM `@CO` line

Every BAM the suite writes (aligner, `deduplicate_bismark`, `filter_non_conversion`,
`methylation_consistency`) also carries a one-line `@CO bismark_provenance` header comment, so the
provenance travels inside the artifact even if the sidecar is separated from it:

```
@CO	bismark_provenance suite=3.0.0 git=abc1234 built=2026-07-11T09:12:00Z run=2026-07-11T14:03:11Z tool=bismark os=linux/x86_64 argv="bismark --genome /g -1 r1.fq -2 r2.fq"
```

Downstream tools copy the input header through, so a deduplicated BAM carries both the aligner's and
`deduplicate_bismark`'s `@CO` lines: a full provenance chain.

### Why does `@PG` still say `VN:v0.25.1`?

The byte-frozen `@PG` line reports `VN:v0.25.1` on purpose: it is the emulated Perl identity that the
byte-identity gates require. The honest build identity is never written into `@PG` (that would break
the gate). Instead it lives in the two places above. To recover the real build that produced a file:

- read the `@CO bismark_provenance` line (`samtools view -H file.bam`, or any BAM reader),
- read the `<output>.bismark_provenance.json` sidecar,
- or, for the container, record the image digest.

The provenance line inside a BAM rides in the header, which no byte-identity gate compares, so it is
byte-safe and always on.

## 5. Test data provenance

The oracle datasets and their public accessions:

| Dataset | Accession |
|---|---|
| WGBS (SE/PE) | SRR24827373 (GSM7445361) |
| Mouse RRBS (PE) | SRR24766921 (GSM7433369) |
| Illumina 5-Base public surrogate (TAPS) | GSE112520 / SRP136786 |

A fully reproducible lambda / pUC19 spike-in gate (public genomes shipped in `test_files/`) provides
a known-truth concordance check that anyone can rerun.

## 6. A one-command smoke test

To confirm the whole toolchain still works and has not silently drifted, run the shipped tiny
fixtures end to end:

```bash
cd rust
just smoke
```

This runs genome preparation and a paired-end alignment on the fixtures in `test_files/`, then
asserts the run succeeds, the report carries its expected structure, and the provenance sidecar (with
the true suite version) landed next to the BAM. It needs Bowtie 2 on `PATH`.
