//! Mandatory run provenance for the Bismark Rust suite (reproducibility-by-design).
//!
//! Every tool that writes a primary analytical output records *how it was produced*,
//! two complementary ways, both ALWAYS ON (no flag):
//!  - a **sidecar JSON** file next to the output — [`write_provenance`]; and
//!  - for BAM outputs, an in-header `@CO` line — [`add_bam_provenance_comment`].
//!
//! Both are byte-identity-safe by construction. The sidecar is a brand-new file Perl
//! never wrote, so it changes zero bytes of any existing output. The `@CO` line rides
//! in the BAM header, which **no** byte-identity gate compares (the aligner is
//! records-only — its `@PG CL:` embeds the `-o` path; `filter_non_conversion` is
//! body-only; `deduplicate_bismark` compares the retained-qname set + report). The
//! frozen `@PG VN:v0.25.1` (the emulated Perl identity) is never touched — the honest
//! suite version, git hash and build/run timestamps live here.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use bstr::BString;
use noodles_sam::Header;

/// Sidecar schema version. Bump on any shape change to the JSON.
pub const SCHEMA_VERSION: u32 = 1;

/// The Perl version whose byte-for-byte output the suite reproduces (recorded so the
/// provenance documents the byte-identity target). Mirrors `aligner::BISMARK_VERSION`
/// et al., which drive the frozen `@PG VN:`; kept separate here so `meta` need not
/// depend on a tool module.
pub const EMULATED_PERL_VERSION: &str = "v0.25.1";

/// The sidecar filename suffix, appended to the primary output's file name.
pub const SIDECAR_SUFFIX: &str = ".bismark_provenance.json";

/// One input file recorded in a provenance record.
pub struct ProvenanceInput {
    pub path: String,
    pub size_bytes: u64,
    /// Seconds since the UNIX epoch (UTC), or `-1` if unavailable.
    pub mtime_epoch_secs: i64,
    // content_sha256: Option<String>  // DEFERRED (see plan): hashing multi-GB inputs on
    // the hot path is a real cost; path+size+mtime is an O(1) stat and always affordable.
    // An absent Option would simply be omitted from the JSON → schema-compatible if added.
}

impl ProvenanceInput {
    /// `stat` a single input: record its path, size, and mtime (best-effort — an
    /// unreadable path yields `size 0`, `mtime -1` rather than failing the run).
    pub fn stat(path: &Path) -> Self {
        let (size_bytes, mtime_epoch_secs) = match std::fs::metadata(path) {
            Ok(m) => {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(-1);
                (m.len(), mtime)
            }
            Err(_) => (0, -1),
        };
        Self {
            path: path.display().to_string(),
            size_bytes,
            mtime_epoch_secs,
        }
    }
}

/// One provenance record. Build-time fields come from [`crate::meta`] consts; the rest
/// are captured at construction (run time). Serialized as deterministic (fixed key
/// order) JSON by [`to_json`].
pub struct ProvenanceRecord<'a> {
    pub schema_version: u32,
    pub tool: &'a str,
    pub suite_version: &'a str,
    pub git_short_hash: &'a str,
    pub build_timestamp: &'a str,
    pub run_timestamp: String,
    pub emulated_perl_version: &'a str,
    pub command_line: Vec<String>,
    pub working_dir: String,
    pub os: &'a str,
    pub arch: &'a str,
    /// The primary output's **file name** (not a full path); the sidecar is written as
    /// `<primary_output><SIDECAR_SUFFIX>` in the same directory.
    pub primary_output: String,
    pub inputs: Vec<ProvenanceInput>,
}

impl<'a> ProvenanceRecord<'a> {
    /// Capture a record for `tool`'s run: build-time provenance from `meta`, run-time
    /// argv / cwd / timestamp / target from the process. `primary_output` is the output
    /// file's name; `inputs` its recorded inputs.
    pub fn new(tool: &'a str, primary_output: String, inputs: Vec<ProvenanceInput>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool,
            suite_version: super::SUITE_VERSION,
            git_short_hash: super::GIT_SHORT_HASH,
            build_timestamp: super::BUILD_TIMESTAMP,
            run_timestamp: now_iso8601_utc(),
            emulated_perl_version: EMULATED_PERL_VERSION,
            command_line: std::env::args().collect(),
            working_dir: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            primary_output,
            inputs,
        }
    }

    fn sidecar_filename(&self) -> String {
        format!("{}{SIDECAR_SUFFIX}", self.primary_output)
    }
}

/// Write `<primary_output><SIDECAR_SUFFIX>` into `output_dir`. **Best-effort**: a write
/// failure logs a warning to stderr and returns — it MUST NOT fail the tool run (the
/// sidecar is auxiliary; it never gates the byte-identical primary output). Call only on
/// the success path, after the primary output is fully flushed/closed.
pub fn write_provenance(output_dir: &Path, record: &ProvenanceRecord) {
    let path = output_dir.join(record.sidecar_filename());
    if let Err(e) = std::fs::write(&path, to_json(record)) {
        eprintln!(
            "bismark: warning: could not write provenance sidecar {}: {e}",
            path.display()
        );
    }
}

/// Convenience: write the sidecar for a single primary output at `output_path`, recording
/// `inputs`. Derives the sidecar's directory + name from `output_path`. Best-effort (see
/// [`write_provenance`]).
pub fn write_for_output(tool: &str, output_path: &Path, inputs: Vec<ProvenanceInput>) {
    let primary_output = output_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = output_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let record = ProvenanceRecord::new(tool, primary_output, inputs);
    write_provenance(dir, &record);
}

/// Convenience: `stat` several input paths into [`ProvenanceInput`]s.
pub fn stat_inputs<I, P>(paths: I) -> Vec<ProvenanceInput>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    paths
        .into_iter()
        .map(|p| ProvenanceInput::stat(p.as_ref()))
        .collect()
}

/// Append the one-line `@CO bismark_provenance …` header comment for `tool` to a BAM
/// header. The line is **run-level** (build-time consts + the process's argv/target and
/// the wall clock) — it carries no per-output field, so it is added once to a header that
/// several outputs may share. noodles serializes `@CO` after `@PG`, so `@HD`/`@SQ`/`@PG`
/// bytes are unchanged. Call at BAM-write time (never inside the frozen
/// `generate_sam_header`).
pub fn add_bam_provenance(header: &mut Header, tool: &str) {
    let line = provenance_line(
        super::SUITE_VERSION,
        super::GIT_SHORT_HASH,
        super::BUILD_TIMESTAMP,
        &now_iso8601_utc(),
        tool,
        std::env::consts::OS,
        std::env::consts::ARCH,
        &std::env::args().collect::<Vec<_>>().join(" "),
    );
    header.comments_mut().push(BString::from(line));
}

/// The `@CO` comment body (without the `@CO\t` prefix noodles adds): a compact,
/// human-readable subset of the sidecar record. Pure formatter for testability.
#[allow(clippy::too_many_arguments)]
fn provenance_line(
    suite: &str,
    git: &str,
    built: &str,
    run: &str,
    tool: &str,
    os: &str,
    arch: &str,
    argv: &str,
) -> String {
    format!(
        "bismark_provenance suite={suite} git={git} built={built} run={run} tool={tool} \
         os={os}/{arch} argv=\"{argv}\""
    )
}

/// Deterministic (fixed key order), pretty-printed JSON for one record, with a trailing
/// newline. Hand-written to avoid a `serde`/`serde_json` dependency (they are reachable
/// only via a dev-dependency and the crate keeps its runtime deps hand-curated).
pub fn to_json(record: &ProvenanceRecord) -> String {
    let mut s = String::with_capacity(512);
    s.push_str("{\n");
    push_u32(&mut s, "schema_version", record.schema_version, false);
    push_str_field(&mut s, "tool", record.tool, false);
    push_str_field(&mut s, "suite_version", record.suite_version, false);
    push_str_field(&mut s, "git_short_hash", record.git_short_hash, false);
    push_str_field(&mut s, "build_timestamp", record.build_timestamp, false);
    push_str_field(&mut s, "run_timestamp", &record.run_timestamp, false);
    push_str_field(
        &mut s,
        "emulated_perl_version",
        record.emulated_perl_version,
        false,
    );

    // "command_line": ["a", "b", ...]
    s.push_str("  \"command_line\": [");
    for (i, a) in record.command_line.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('"');
        json_escape_into(&mut s, a);
        s.push('"');
    }
    s.push_str("],\n");

    push_str_field(&mut s, "working_dir", &record.working_dir, false);
    push_str_field(&mut s, "os", record.os, false);
    push_str_field(&mut s, "arch", record.arch, false);
    push_str_field(&mut s, "primary_output", &record.primary_output, false);

    // "inputs": [ { ... }, ... ]  (last field → no trailing comma)
    s.push_str("  \"inputs\": [");
    if record.inputs.is_empty() {
        s.push_str("]\n");
    } else {
        s.push('\n');
        for (i, inp) in record.inputs.iter().enumerate() {
            s.push_str("    {\"path\": \"");
            json_escape_into(&mut s, &inp.path);
            s.push_str(&format!(
                "\", \"size_bytes\": {}, \"mtime_epoch_secs\": {}}}",
                inp.size_bytes, inp.mtime_epoch_secs
            ));
            if i + 1 < record.inputs.len() {
                s.push(',');
            }
            s.push('\n');
        }
        s.push_str("  ]\n");
    }

    s.push_str("}\n");
    s
}

fn push_str_field(s: &mut String, key: &str, val: &str, last: bool) {
    s.push_str("  \"");
    s.push_str(key);
    s.push_str("\": \"");
    json_escape_into(s, val);
    s.push('"');
    s.push_str(if last { "\n" } else { ",\n" });
}

fn push_u32(s: &mut String, key: &str, val: u32, last: bool) {
    s.push_str(&format!(
        "  \"{key}\": {val}{}",
        if last { "\n" } else { ",\n" }
    ));
}

/// Escape a string into a JSON string body (the surrounding quotes are the caller's).
fn json_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

// ── Runtime UTC timestamp ────────────────────────────────────────────────────
// Ported from `build.rs` (which formats the *build* timestamp): the same
// dependency-free formatter, applied to the wall clock at run time so the sidecar's
// `run_timestamp` has the identical ISO-8601 UTC shape as `BUILD_TIMESTAMP`.

fn now_iso8601_utc() -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_iso8601_utc(epoch)
}

fn format_iso8601_utc(epoch: u64) -> String {
    let secs_of_day = epoch % 86_400;
    let days = epoch / 86_400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

// Howard Hinnant's days-from-civil algorithm (public domain). Days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y + i64::from(m <= 2);
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record with fully deterministic (injected) values so JSON/`@CO` assertions do
    /// not depend on the wall clock, cwd, or argv of the test process.
    fn fixed_record() -> ProvenanceRecord<'static> {
        ProvenanceRecord {
            schema_version: SCHEMA_VERSION,
            tool: "bismark",
            suite_version: "3.0.0",
            git_short_hash: "abc1234",
            build_timestamp: "2026-07-11T09:12:00Z",
            run_timestamp: "2026-07-11T14:03:11Z".to_string(),
            emulated_perl_version: EMULATED_PERL_VERSION,
            command_line: vec![
                "bismark".to_string(),
                "--genome".to_string(),
                "/g".to_string(),
                "reads.fq".to_string(),
            ],
            working_dir: "/work".to_string(),
            os: "linux",
            arch: "x86_64",
            primary_output: "sample_bismark_bt2.bam".to_string(),
            inputs: vec![ProvenanceInput {
                path: "reads.fq".to_string(),
                size_bytes: 12_345,
                mtime_epoch_secs: 1_700_000_000,
            }],
        }
    }

    #[test]
    fn json_is_deterministic() {
        let r = fixed_record();
        assert_eq!(to_json(&r), to_json(&r));
    }

    #[test]
    fn json_exact_shape() {
        let expected = "\
{
  \"schema_version\": 1,
  \"tool\": \"bismark\",
  \"suite_version\": \"3.0.0\",
  \"git_short_hash\": \"abc1234\",
  \"build_timestamp\": \"2026-07-11T09:12:00Z\",
  \"run_timestamp\": \"2026-07-11T14:03:11Z\",
  \"emulated_perl_version\": \"v0.25.1\",
  \"command_line\": [\"bismark\", \"--genome\", \"/g\", \"reads.fq\"],
  \"working_dir\": \"/work\",
  \"os\": \"linux\",
  \"arch\": \"x86_64\",
  \"primary_output\": \"sample_bismark_bt2.bam\",
  \"inputs\": [
    {\"path\": \"reads.fq\", \"size_bytes\": 12345, \"mtime_epoch_secs\": 1700000000}
  ]
}
";
        assert_eq!(to_json(&fixed_record()), expected);
    }

    #[test]
    fn json_escapes_special_chars() {
        let mut r = fixed_record();
        r.working_dir = "a\"b\\c\nd\te".to_string();
        let json = to_json(&r);
        assert!(json.contains(r#""working_dir": "a\"b\\c\nd\te""#));
    }

    #[test]
    fn json_empty_inputs_is_valid() {
        let mut r = fixed_record();
        r.inputs.clear();
        let json = to_json(&r);
        assert!(json.contains("\"inputs\": []\n"));
        assert!(json.ends_with("}\n"));
    }

    #[test]
    fn provenance_line_shape() {
        let line = provenance_line(
            "3.0.0",
            "abc1234",
            "2026-07-11T09:12:00Z",
            "2026-07-11T14:03:11Z",
            "bismark",
            "linux",
            "x86_64",
            "bismark --genome /g reads.fq",
        );
        assert_eq!(
            line,
            "bismark_provenance suite=3.0.0 git=abc1234 built=2026-07-11T09:12:00Z \
             run=2026-07-11T14:03:11Z tool=bismark os=linux/x86_64 \
             argv=\"bismark --genome /g reads.fq\""
        );
    }

    #[test]
    fn bam_comment_added_after_pg() {
        // @CO must serialize last; the record stream + @HD/@SQ/@PG are unaffected.
        let mut header = Header::default();
        add_bam_provenance(&mut header, "bismark");
        assert_eq!(header.comments().len(), 1);
        assert!(header.comments()[0].starts_with(b"bismark_provenance "));
    }

    #[test]
    fn iso8601_formatter_matches_known_epoch() {
        // 1700000000 = 2023-11-14T22:13:20Z (the SOURCE_DATE_EPOCH used by `just reproduce`).
        assert_eq!(format_iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn sidecar_filename_appends_suffix() {
        assert_eq!(
            fixed_record().sidecar_filename(),
            "sample_bismark_bt2.bam.bismark_provenance.json"
        );
    }
}
