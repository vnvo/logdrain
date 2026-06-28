//! `logdrain` CLI: read log lines from files or stdin, mine templates, and print
//! the resulting clusters as text, JSON, or CSV.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use logdrain::{builtin_masks, Mask, Miner};
use logdrain_cli::format::{self, Format, Sort};
use logdrain_cli::input;
use regex::Regex;

/// Read buffer size for `--record-separator` streaming.
const SEP_CHUNK: usize = 64 * 1024;

#[derive(Parser, Debug)]
#[command(name = "logdrain", version, about = "Online log-template miner")]
struct Cli {
    /// Input files. Reads stdin if none are given.
    files: Vec<PathBuf>,

    /// JSON field to extract from each line (dot notation, e.g. `event.type`).
    /// When unset, each line is treated as raw text.
    #[arg(long)]
    key: Option<String>,

    /// Output format.
    #[arg(long, default_value = "text")]
    format: Format,

    /// Hide clusters smaller than N.
    #[arg(long, default_value_t = 0)]
    min_size: u64,

    /// Sort order.
    #[arg(long, default_value = "size")]
    sort: Sort,

    /// Similarity threshold in [0.0, 1.0].
    #[arg(long = "sim-th", default_value_t = 0.4)]
    sim_th: f64,

    /// Prefix-tree depth (>= 2).
    #[arg(long, default_value_t = 4)]
    depth: usize,

    /// Delimiter characters preserved as token boundaries, e.g. `/`.
    #[arg(long = "path-delimiters")]
    path_delimiters: Option<String>,

    /// Cluster on the first line only (keep the rest as a suffix).
    #[arg(long = "first-line-only")]
    first_line_only: bool,

    /// Comma-separated built-in masks: uuid,hex32,email,ipv4,jwt.
    #[arg(long)]
    masks: Option<String>,

    /// Split input on this literal separator instead of newlines, so each chunk
    /// (newlines and all) is one record. Use when records are *explicitly framed*
    /// in the stream. Escapes `\n \r \t \0 \\` are interpreted, e.g. `\n\n` for
    /// blank-line-separated records or `\0` for NUL-delimited streams.
    /// Mutually exclusive with --multiline-start.
    #[arg(long = "record-separator", value_name = "SEP")]
    record_separator: Option<String>,

    /// Start a new record at every line matching this regex; lines that do not
    /// match are appended to the current record. Use for *delimiter-less* logs
    /// where a record begins with a recognizable line (timestamp, level) and
    /// continuation lines (stack frames) follow, e.g. `^\d{4}-\d{2}-\d{2}` or
    /// `^(ERROR|WARN|INFO)`. Mutually exclusive with --record-separator.
    #[arg(long = "multiline-start", value_name = "REGEX")]
    multiline_start: Option<String>,

    /// JSON dot-path to a per-record event timestamp (e.g. `ts` or `meta.time`).
    /// Enables true event-time rates instead of processing wall-clock: it drives
    /// the `eventFirstSeen` / `eventLastSeen` / `eventRatePerMinute` output fields.
    /// Records whose value is missing or unparseable are still clustered, just
    /// without event time (a count is reported on stderr).
    #[arg(long = "time-key", value_name = "FIELD")]
    time_key: Option<String>,

    /// chrono format for parsing the --time-key value, e.g. `%Y-%m-%d %H:%M:%S`.
    /// Without it, integer values are read as a Unix epoch (seconds/millis/micros/
    /// nanos, auto-scaled) and strings as RFC 3339. Requires --time-key.
    #[arg(long = "time-format", value_name = "FMT", requires = "time_key")]
    time_format: Option<String>,
}

/// Interpret the common backslash escapes in a user-supplied separator so values
/// like `\n\n` or `\0` can be passed as plain argv text.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// First index of `needle` within `hay`, or `None`. `needle` is assumed non-empty.
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn build_masks(spec: &str) -> Result<Vec<Mask>, String> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| match name {
            "uuid" => Ok(builtin_masks::uuid()),
            "hex32" => Ok(builtin_masks::hex32()),
            "email" => Ok(builtin_masks::email()),
            "ipv4" => Ok(builtin_masks::ipv4()),
            "jwt" => Ok(builtin_masks::jwt()),
            other => Err(format!(
                "unknown mask '{other}' (known: uuid, hex32, email, ipv4, jwt)"
            )),
        })
        .collect()
}

fn build_miner(cli: &Cli) -> Result<Miner, String> {
    let mut b = Miner::builder()
        .sim_threshold(cli.sim_th)
        .depth(cli.depth)
        .first_line_only(cli.first_line_only);
    if let Some(pd) = &cli.path_delimiters {
        let chars: Vec<char> = pd.chars().collect();
        b = b.path_delimiters(&chars);
    }
    if let Some(spec) = &cli.masks {
        b = b.masks(build_masks(spec)?);
    }
    b.build().map_err(|e| e.to_string())
}

/// Parse a timestamp value into unix-milliseconds.
///
/// With an explicit `format` (chrono spec, e.g. `%Y-%m-%d %H:%M:%S`): try an
/// offset-aware parse first, then a naive parse interpreted as UTC. Without a
/// format: a bare integer is a Unix epoch (seconds / millis / micros / nanos,
/// auto-scaled by magnitude); a string is parsed as RFC 3339. Returns `None` if
/// nothing matches.
fn parse_event_ms(value: &str, format: Option<&str>) -> Option<u64> {
    let value = value.trim();
    if let Some(fmt) = format {
        if let Ok(dt) = chrono::DateTime::parse_from_str(value, fmt) {
            return u64::try_from(dt.timestamp_millis()).ok();
        }
        let ndt = chrono::NaiveDateTime::parse_from_str(value, fmt).ok()?;
        return u64::try_from(ndt.and_utc().timestamp_millis()).ok();
    }
    if let Ok(n) = value.parse::<i64>() {
        return scale_epoch(n);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|dt| u64::try_from(dt.timestamp_millis()).ok())
}

/// Scale a bare epoch integer to milliseconds by magnitude (seconds / millis /
/// micros / nanos). Rejects negatives.
fn scale_epoch(n: i64) -> Option<u64> {
    let n = u64::try_from(n).ok()?;
    let ms = if n < 100_000_000_000 {
        n * 1000 // seconds      (< 1e11)
    } else if n < 100_000_000_000_000 {
        n // milliseconds (< 1e14)
    } else if n < 100_000_000_000_000_000 {
        n / 1000 // microseconds (< 1e17)
    } else {
        n / 1_000_000 // nanoseconds
    };
    Some(ms)
}

/// Per-run ingestion config: how to pull the message and (optionally) the event
/// timestamp out of each record, plus a tally of timestamp parse failures.
struct Ingestor<'a> {
    miner: &'a Miner,
    key: Option<&'a str>,
    time_key: Option<&'a str>,
    time_format: Option<&'a str>,
    time_parse_failures: Cell<u64>,
}

impl Ingestor<'_> {
    /// Feed one complete record to the miner: extract the message (`--key`), the
    /// event time (`--time-key`), and add.
    fn feed(&self, record: &str) {
        let event_ms = self.event_ms(record);
        match self.key {
            Some(k) => {
                if let Some(msg) = input::extract_field(record, k) {
                    self.add(&msg, event_ms);
                }
            }
            None => {
                if !record.trim().is_empty() {
                    self.add(record, event_ms);
                }
            }
        }
    }

    fn add(&self, msg: &str, event_ms: Option<u64>) {
        match event_ms {
            Some(ms) => self.miner.add_at(msg, ms),
            None => self.miner.add(msg),
        };
    }

    /// Event timestamp for a record, or `None`. Counts a failure when `--time-key`
    /// is set but the field is missing or unparseable; silent when it is unset.
    fn event_ms(&self, record: &str) -> Option<u64> {
        let tk = self.time_key?;
        let parsed =
            input::extract_field(record, tk).and_then(|v| parse_event_ms(&v, self.time_format));
        if parsed.is_none() {
            self.time_parse_failures
                .set(self.time_parse_failures.get() + 1);
        }
        parsed
    }
}

/// Default: one physical line per record.
fn ingest_reader(ing: &Ingestor, reader: impl BufRead) -> io::Result<()> {
    for line in reader.lines() {
        ing.feed(&line?);
    }
    Ok(())
}

/// `--multiline-start`: a line matching `start` opens a new record; non-matching
/// lines are appended (with their newline) to the record in progress. Memory is
/// bounded by the size of a single record.
fn ingest_multiline_start(ing: &Ingestor, reader: impl BufRead, start: &Regex) -> io::Result<()> {
    let mut record = String::new();
    for line in reader.lines() {
        let line = line?;
        if start.is_match(&line) && !record.is_empty() {
            ing.feed(&record);
            record.clear();
        }
        if !record.is_empty() {
            record.push('\n');
        }
        record.push_str(&line);
    }
    if !record.is_empty() {
        ing.feed(&record);
    }
    Ok(())
}

/// `--record-separator`: split the byte stream on `sep`; each piece between
/// separators (newlines and all) is one record. Streamed in fixed chunks, so
/// memory is bounded by the size of a single record plus one chunk.
fn ingest_separated(ing: &Ingestor, mut reader: impl Read, sep: &[u8]) -> io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; SEP_CHUNK];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        while let Some(pos) = find_sub(&buf, sep) {
            let record: Vec<u8> = buf.drain(..pos + sep.len()).collect();
            ing.feed(&String::from_utf8_lossy(&record[..pos]));
        }
    }
    if !buf.is_empty() {
        ing.feed(&String::from_utf8_lossy(&buf));
    }
    Ok(())
}

/// Route one input source to the active record-assembly strategy.
fn ingest_source(
    ing: &Ingestor,
    reader: impl BufRead,
    sep: Option<&[u8]>,
    start: Option<&Regex>,
) -> io::Result<()> {
    match (sep, start) {
        (Some(s), _) => ingest_separated(ing, reader, s),
        (_, Some(re)) => ingest_multiline_start(ing, reader, re),
        (None, None) => ingest_reader(ing, reader),
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let miner = build_miner(&cli)?;

    if cli.record_separator.is_some() && cli.multiline_start.is_some() {
        return Err("--record-separator and --multiline-start are mutually exclusive".into());
    }
    let sep = match &cli.record_separator {
        Some(s) => {
            let s = unescape(s);
            if s.is_empty() {
                return Err("--record-separator must not be empty".into());
            }
            Some(s.into_bytes())
        }
        None => None,
    };
    let start = match &cli.multiline_start {
        Some(p) => {
            Some(Regex::new(p).map_err(|e| format!("invalid --multiline-start regex: {e}"))?)
        }
        None => None,
    };
    let sep = sep.as_deref();
    let start = start.as_ref();

    let ing = Ingestor {
        miner: &miner,
        key: cli.key.as_deref(),
        time_key: cli.time_key.as_deref(),
        time_format: cli.time_format.as_deref(),
        time_parse_failures: Cell::new(0),
    };

    if cli.files.is_empty() {
        let stdin = io::stdin();
        ingest_source(&ing, stdin.lock(), sep, start).map_err(|e| e.to_string())?;
    } else {
        for path in &cli.files {
            let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            ingest_source(&ing, BufReader::new(f), sep, start).map_err(|e| e.to_string())?;
        }
    }

    let failures = ing.time_parse_failures.get();
    if failures > 0 {
        eprintln!(
            "logdrain: warning: {failures} record(s) had a missing or unparseable \
             --time-key value (clustered without event time)"
        );
    }

    let clusters = format::prepare(miner.clusters(), cli.min_size, cli.sort);
    print!("{}", format::render(&clusters, cli.format));
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("logdrain: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ingestor with no `--key` / time options, for the simple cases.
    fn plain(m: &Miner) -> Ingestor<'_> {
        Ingestor {
            miner: m,
            key: None,
            time_key: None,
            time_format: None,
            time_parse_failures: Cell::new(0),
        }
    }

    #[test]
    fn build_masks_parses_known_and_rejects_unknown() {
        assert_eq!(build_masks("uuid,ipv4").unwrap().len(), 2);
        assert!(build_masks("").unwrap().is_empty());
        assert!(build_masks("uuid,bogus").is_err());
    }

    #[test]
    fn feed_skips_blank_extracts_json_and_adds_raw() {
        let m = Miner::builder().build().unwrap();
        let raw = plain(&m);
        raw.feed("   "); // blank -> skipped
        assert_eq!(m.len(), 0);
        raw.feed("plain text line"); // raw -> added
        assert_eq!(m.len(), 1);

        let m2 = Miner::builder().build().unwrap();
        let keyed = Ingestor {
            miner: &m2,
            key: Some("msg"),
            time_key: None,
            time_format: None,
            time_parse_failures: Cell::new(0),
        };
        keyed.feed(r#"{"msg":"hello world"}"#); // JSON field extracted
        assert!(m2.match_only("hello world").is_some());
        keyed.feed("not json"); // unparseable under --key -> skipped
        assert!(m2.match_only("not json").is_none());
    }

    #[test]
    fn unescape_interprets_common_escapes_and_preserves_unknown() {
        assert_eq!(unescape(r"\n\n"), "\n\n");
        assert_eq!(unescape(r"\0"), "\0");
        assert_eq!(unescape(r"\t\r"), "\t\r");
        assert_eq!(unescape(r"a\\b"), "a\\b");
        assert_eq!(unescape("plain"), "plain");
        assert_eq!(unescape(r"\x"), r"\x"); // unknown escape kept verbatim
        assert_eq!(unescape(r"trailing\"), r"trailing\"); // lone backslash kept
    }

    #[test]
    fn find_sub_locates_needle() {
        assert_eq!(find_sub(b"abcXYdef", b"XY"), Some(3));
        assert_eq!(find_sub(b"\0a\0", b"\0"), Some(0));
        assert_eq!(find_sub(b"abc", b"Z"), None);
    }

    #[test]
    fn multiline_start_groups_continuation_lines_into_one_record() {
        let m = Miner::builder().first_line_only(true).build().unwrap();
        let re = Regex::new("^ERROR").unwrap();
        // Two ERROR records, each with indented continuation frames.
        let input = "ERROR boom\n\tat a\n\tat b\nERROR boom\n\tat c\n";
        ingest_multiline_start(&plain(&m), input.as_bytes(), &re).unwrap();

        assert_eq!(m.len(), 1, "both records share the first line");
        let c = &m.clusters()[0];
        assert_eq!(c.size(), 2);
        assert_eq!(c.suffix(), Some("\tat a\n\tat b")); // continuation kept as suffix
    }

    #[test]
    fn record_separator_splits_on_delimiter() {
        let m = Miner::builder().first_line_only(true).build().unwrap();
        // Two NUL-framed records, each a multi-line trace; trailing separator present.
        let input = b"ERROR boom\n\tat a\0ERROR boom\n\tat b\0";
        ingest_separated(&plain(&m), &input[..], b"\0").unwrap();

        assert_eq!(m.len(), 1);
        assert_eq!(m.clusters()[0].size(), 2);
    }

    #[test]
    fn record_separator_flushes_trailing_record_without_terminator() {
        let m = Miner::builder().build().unwrap();
        // Blank-line separator, last record not terminated by one.
        let input = b"alpha\n\nbeta\n\ngamma";
        ingest_separated(&plain(&m), &input[..], b"\n\n").unwrap();
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn scale_epoch_detects_unit_by_magnitude() {
        assert_eq!(scale_epoch(1_700_000_000), Some(1_700_000_000_000)); // seconds
        assert_eq!(scale_epoch(1_700_000_000_000), Some(1_700_000_000_000)); // millis
        assert_eq!(scale_epoch(1_700_000_000_000_000), Some(1_700_000_000_000)); // micros
        assert_eq!(
            scale_epoch(1_700_000_000_000_000_000),
            Some(1_700_000_000_000)
        ); // nanos
        assert_eq!(scale_epoch(-1), None);
    }

    #[test]
    fn parse_event_ms_autodetects_and_honors_format() {
        assert_eq!(parse_event_ms("60", None), Some(60_000)); // epoch seconds
        assert_eq!(parse_event_ms("1970-01-01T00:01:00Z", None), Some(60_000)); // RFC 3339
        assert_eq!(
            parse_event_ms("1970-01-01 00:01:00", Some("%Y-%m-%d %H:%M:%S")),
            Some(60_000)
        ); // explicit format, naive -> UTC
        assert_eq!(parse_event_ms("not a time", None), None);
        assert_eq!(parse_event_ms("nope", Some("%Y")), None);
    }

    #[test]
    fn feed_with_time_key_tracks_window_and_counts_failures() {
        use std::time::{Duration, UNIX_EPOCH};
        let m = Miner::builder().build().unwrap();
        let ing = Ingestor {
            miner: &m,
            key: Some("msg"),
            time_key: Some("ts"),
            time_format: None,
            time_parse_failures: Cell::new(0),
        };
        ing.feed(r#"{"msg":"user 1 in","ts":60}"#); // event t = 60s
        ing.feed(r#"{"msg":"user 2 in","ts":180}"#); // joins, t = 180s
        ing.feed(r#"{"msg":"user 3 in"}"#); // missing ts -> failure, still clustered

        let cs = m.clusters();
        assert_eq!(cs.len(), 1);
        let c = &cs[0];
        assert_eq!(c.size(), 3);
        assert_eq!(
            c.event_first_seen(),
            Some(UNIX_EPOCH + Duration::from_secs(60))
        );
        assert_eq!(
            c.event_last_seen(),
            Some(UNIX_EPOCH + Duration::from_secs(180))
        );
        assert_eq!(ing.time_parse_failures.get(), 1);
    }
}
