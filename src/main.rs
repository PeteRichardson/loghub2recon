//! `loghub2recon`: a loghub event-template CSV becomes a recon filter set.
//!
//! loghub (<https://github.com/logpai/loghub>) ships 16 sample logs, and beside
//! each one a CSV of the event templates that a log parser found in it. A
//! template is a mask of one message body — `Thermal pressure state: <*>
//! Memory pressure state: <*>` — where `<*>` marks the position that held
//! instance-specific data. That is very close to a recon filter, and this tool
//! does the last step: it reads the CSV and writes a `filters.toml` fragment
//! with one `[[sets.<Name>.filters]]` entry per template.
//!
//! It is a repo of its own because it is a one-time job. Nothing in recon
//! calls it, it takes no terminal, and a conversion is something you do once
//! per dataset and then keep the file. `cargo run -- Mac`.
//!
//! The conversion rules, and why each one is what it is, are in
//! `docs/loghub-template-format.md`. Three of them decide this file:
//!
//! - **Split on the exact token `<*>`, never on `<[^>]*>`.** Templates hold
//!   literal angle brackets — `<ok>`, `<<<< Boss >>>>`, `< FigByteStream >` —
//!   and a greedy substitution destroys all of them. `<NUM>` is a second
//!   spelling of the wildcard and is treated as one.
//! - **Everything that is not a wildcard is literal**, `*`, `.`, `(`, `[`, `+`,
//!   `|`, `?`, `$` and `\` included, so each literal run goes through
//!   `regex::escape`. A run of whitespace becomes `\s+`, because the ground
//!   truth has lines whose spacing does not match the template's.
//! - **No `^` and no `$`.** loghub's own matcher anchors, because it matches
//!   against a message body that the log format has already had its timestamp,
//!   host, component and PID cut from. recon has no such format and matches a
//!   whole raw line with `Regex::is_match`, so an anchored pattern would never
//!   fire. See `pattern_for`, which is where that difference is paid for.
//!
//! What the emitted set deliberately does *not* carry: `priority`, `autoload`,
//! `colour`, and any `profiles` table. Each is a one-line hand edit to a file
//! this tool has just shown the shape of, and a default nobody asked for is a
//! thing to delete later — the same reasoning recon's own `S` follows in its
//! `docs/specs/2026-09-03-saved-filter-sets-design.md`, *Saving*. A set with
//! no `default` profile opens as a header row over a column of `[ ]`, which is
//! the right starting point for a hundred-odd templates: you pick the handful
//! that describe the thing you are chasing.
//!
//! ## What this repo knows about recon, and how it stays true
//!
//! This tool lived inside recon until it was split out, and there it validated
//! its output by calling recon's own `filtersets::parse` — the function recon
//! reads `filters.toml` with. That was the best thing about it, and a
//! dependency edge back to recon would not pay for itself: `filtersets` reaches
//! into recon's colour parser and its syntax kinds, so a git dependency would
//! compile ratatui, crossterm, syntect and two-face to convert a CSV.
//!
//! [`validate`] is the stand-in, and it is equivalent *for what this tool
//! emits*. Everything recon's loader can refuse in one of these files is
//! checked there. Every rule it does not check — `colour`, `sense`, `mode`,
//! `profiles`, the built-in sets' restrictions — is unreachable, because this
//! tool never writes those keys. [`validate`] says which recon that was true
//! of; when recon's schema moves, that is the function to revisit.

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::{bail, eyre};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The 16 loghub datasets that ship both a log and a template file. Hard-coded
/// rather than discovered, so that a misspelt name is refused locally with the
/// list in the message instead of costing a round trip and a 404.
const DATASETS: [&str; 16] = [
    "Android",
    "Apache",
    "BGL",
    "HDFS",
    "HPC",
    "Hadoop",
    "HealthApp",
    "Linux",
    "Mac",
    "OpenSSH",
    "OpenStack",
    "Proxifier",
    "Spark",
    "Thunderbird",
    "Windows",
    "Zookeeper",
];

/// The two spellings of a wildcard. `<NUM>` occurs only in Mac `E168`–`E171`,
/// and the structured ground truth writes `<*>` for those same four events, so
/// it is the same thing under another name. `<ID>` and `<IP>` never occur.
const WILDCARDS: [&str; 2] = ["<*>", "<NUM>"];

/// What a wildcard becomes. Lazy rather than greedy to follow the spec's
/// recipe; for `is_match` the two give the same answer, and `.*?` is what the
/// documented rule says, so the code and the spec do not have to be reconciled
/// by a reader later.
const WILDCARD_PATTERN: &str = ".*?";

/// The point above which recon's navigator stops matching patterns across the
/// folder. A converted dataset is very often over it, so the tool says so
/// rather than leaving it to be discovered.
const NAVIGATOR_CEILING: usize = 64;

/// recon's built-in set names. A table under one of these may carry no
/// `filters`, so emitting a set called `definitions` would produce a file
/// recon refuses to start on.
///
/// Copied rather than imported — see the note at the top of this file about
/// what this repo knows about recon. It is one name today; `validate` is where
/// a second one would also have to be taught.
const BUILTIN_SETS: [&str; 1] = ["definitions"];

/// loghub serves the CSV from `raw.githubusercontent.com`. The
/// `github.com/logpai/loghub/<Name>/...` form is a web page, not a file: it
/// 404s, and when it does not it returns HTML.
fn template_url(name: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/logpai/loghub/master/{name}/{name}_2k.log_templates.csv"
    )
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Convert a loghub event-template CSV into a recon filter set"
)]
struct Cli {
    /// A path to a loghub `*_templates.csv`, or the name of a loghub log type
    /// — `Mac`, `OpenSSH`, `Windows` — whose templates are then fetched from
    /// GitHub. A name is matched without regard to case.
    #[arg(value_name = "CSV_OR_NAME", required_unless_present = "list")]
    template: Option<String>,

    /// List the log types that may be named instead of a path, and exit.
    #[arg(long)]
    list: bool,

    /// The set's name in the emitted file. Defaults to the log type's name,
    /// or to the leading part of the CSV's file name.
    #[arg(long, value_name = "NAME")]
    set_name: Option<String>,

    /// Where to write. `-` is stdout. Defaults to `<set>-filters.toml` in the
    /// current directory.
    #[arg(short, long, value_name = "PATH")]
    output: Option<String>,

    /// Overwrite the output file if it is already there.
    #[arg(long)]
    force: bool,
}

fn main() -> Result<()> {
    // No location section and no backtrace note. Nearly every error this tool
    // raises is something the user typed — a name that is not a dataset, a
    // file that is not there — and a source line with a backtrace offer under
    // it says "you found a bug", which is the wrong thing to tell them. The
    // one error that *is* a bug says so in its own text.
    color_eyre::config::HookBuilder::default()
        .display_location_section(false)
        .display_env_section(false)
        .install()?;
    let cli = Cli::parse();

    if cli.list {
        for name in DATASETS {
            println!("{name}");
        }
        return Ok(());
    }

    let argument = cli
        .template
        .as_deref()
        .ok_or_else(|| eyre!("no CSV or log type given"))?;
    let source = Source::resolve(argument)?;
    let csv = source.read()?;
    let set = match cli.set_name {
        Some(name) => name,
        None => source.default_set_name(),
    };
    if set.is_empty() {
        bail!("the set's name is empty; give one with --set-name");
    }
    if BUILTIN_SETS.contains(&set.as_str()) {
        bail!(
            "{set:?} is one of recon's built-in set names, which may not carry \
             filters; give another with --set-name"
        );
    }

    let templates = read_templates(&csv, &source.describe())?;
    let outcome = convert(&templates);
    if outcome.filters.is_empty() {
        bail!(
            "{}: no template produced a usable pattern",
            source.describe()
        );
    }
    let text = emit(&set, &outcome.filters, &source)?;

    // Proof rather than hope: "recon will load this" is established here and
    // not at the user's next start. A failure is this tool's bug and says so.
    let target = cli
        .output
        .clone()
        .unwrap_or_else(|| format!("{set}-filters.toml"));
    validate(&text, &set)?;

    if target == "-" {
        print!("{text}");
    } else {
        let path = PathBuf::from(&target);
        if path.exists() && !cli.force {
            bail!(
                "{} is already there; pass --force to overwrite",
                path.display()
            );
        }
        std::fs::write(&path, &text)
            .map_err(|err| eyre!("could not write {}: {err}", path.display()))?;
        eprintln!("wrote {}", path.display());
    }

    report(&outcome, &set, &target);
    Ok(())
}

// ---- where the CSV comes from ------------------------------------------

/// The two ways to name a template file, resolved once so that everything
/// after it — the default set name, the provenance comment, the error
/// messages — reads one value instead of re-deciding.
#[derive(Debug)]
enum Source {
    /// A file on this machine.
    Local(PathBuf),
    /// A loghub dataset, by its canonical name.
    Dataset(&'static str),
}

impl Source {
    /// A path if it exists or looks like one; otherwise a dataset name.
    ///
    /// "Looks like one" matters for the message. Without it a mistyped path
    /// falls through to the dataset branch and is refused with a list of 16
    /// log types, which is not what the user got wrong.
    fn resolve(argument: &str) -> Result<Self> {
        let path = Path::new(argument);
        if path.exists() {
            return Ok(Self::Local(path.to_path_buf()));
        }
        if let Some(name) = DATASETS
            .iter()
            .find(|name| name.eq_ignore_ascii_case(argument))
        {
            return Ok(Self::Dataset(name));
        }
        if looks_like_a_path(argument) {
            bail!("no such file: {argument}");
        }
        bail!(
            "{argument:?} is neither a file nor a loghub log type. The log types are: {}",
            DATASETS.join(", ")
        )
    }

    /// The CSV's text. Invalid UTF-8 is replaced rather than refused: the
    /// templates are ASCII — `LogLoader` replaces every run of non-ASCII bytes
    /// with the literal `<N/ASCII>` before a parser ever sees a line — so a
    /// stray byte is damage in one row, not a reason to convert nothing.
    fn read(&self) -> Result<String> {
        let bytes = match self {
            Self::Local(path) => std::fs::read(path)
                .map_err(|err| eyre!("could not read {}: {err}", path.display()))?,
            Self::Dataset(name) => fetch(&template_url(name))?,
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// What the set is called when `--set-name` does not say.
    ///
    /// For a dataset that is its name. For a file it is the file name up to
    /// `_2k`, which turns both `Mac_2k.log_templates.csv` and
    /// `Mac_2k.log_templates_corrected.csv` into `Mac`; a file named anything
    /// else keeps its stem.
    fn default_set_name(&self) -> String {
        match self {
            Self::Dataset(name) => (*name).to_string(),
            Self::Local(path) => {
                let file = path
                    .file_name()
                    .map_or(String::new(), |name| name.to_string_lossy().into_owned());
                match file.find("_2k") {
                    Some(at) => file[..at].to_string(),
                    None => file
                        .rsplit_once('.')
                        .map_or(file.clone(), |(stem, _)| stem.to_string()),
                }
            }
        }
    }

    /// Where the templates came from, for messages and for the file's header
    /// comment. A converted set outlives the command that made it, so the file
    /// has to say what it was made from.
    fn describe(&self) -> String {
        match self {
            Self::Local(path) => path.display().to_string(),
            Self::Dataset(name) => template_url(name),
        }
    }
}

/// Whether the argument is a path the user mistyped rather than a log type
/// they mistyped. A separator or a `.csv` suffix is the whole test — anything
/// finer would start guessing.
fn looks_like_a_path(argument: &str) -> bool {
    argument.contains('/')
        || argument.contains('\\')
        || argument.to_ascii_lowercase().ends_with(".csv")
}

/// Fetch a URL with `curl`.
///
/// A spawned `curl` rather than an HTTP crate: recon has no network
/// dependency, and this tool runs by hand a handful of times in a dataset's
/// life. An in-process client would put a TLS stack and its transitive crates
/// into the build of a log viewer that never opens a socket. recon already
/// spawns processes for the editor and the clipboard, so this is the
/// established shape here and not a new one.
fn fetch(url: &str) -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", url])
        .output()
        .map_err(|err| {
            eyre!(
                "could not run `curl`: {err}. Download the CSV yourself and pass its \
                 path instead: {url}"
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("could not fetch {url}: {}", stderr.trim());
    }
    Ok(output.stdout)
}

// ---- reading the CSV ----------------------------------------------------

/// The `EventTemplate` values, in the file's own order.
///
/// A real CSV parser, not a line reader: Mac `E152` and `E281` each hold an
/// embedded newline, so `Mac_2k.log_templates.csv` has more physical lines
/// than rows, and Thunderbird's corrected file has a field that spans six
/// lines. `flexible(true)` because some producers write a third `Occurrences`
/// column; the column is found by name, so an extra one is ignored.
fn read_templates(csv: &str, source: &str) -> Result<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(csv.as_bytes());
    let headers = reader
        .headers()
        .map_err(|err| eyre!("{source}: could not read the header row: {err}"))?
        .clone();
    let column = headers
        .iter()
        .position(|name| name.trim() == "EventTemplate")
        .ok_or_else(|| {
            eyre!(
                "{source}: no `EventTemplate` column; the header row is {:?}",
                headers.iter().collect::<Vec<_>>()
            )
        })?;

    let mut templates = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| eyre!("{source}: {err}"))?;
        // An empty cell is `NaN` to pandas and a panic to logparser. Here it
        // is simply a row with nothing to convert.
        if let Some(template) = record.get(column)
            && !template.trim().is_empty()
        {
            templates.push(template.to_string());
        }
    }
    if templates.is_empty() {
        bail!("{source}: the `EventTemplate` column is empty");
    }
    Ok(templates)
}

// ---- the conversion -----------------------------------------------------

/// One piece of a split template.
#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Literal(&'a str),
    Wildcard,
}

/// What a conversion produced, and what it threw away. The counts are the
/// report: a set of 158 filters from a file of 166 rows should say where the
/// other 8 went.
#[derive(Debug, Default)]
struct Outcome {
    /// `(name, pattern)` pairs, in the CSV's order.
    filters: Vec<(String, String)>,
    /// Templates with no literal text at all — `<*>`, `<*> <*>`. A pattern
    /// from one of these matches every line, which is not a filter.
    empty: usize,
    /// Templates whose pattern another template had already produced.
    duplicate: usize,
}

/// Split a template on the exact wildcard tokens, keeping both sides.
fn split_template(template: &str) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let mut rest = template;
    loop {
        // The earliest wildcard, whichever spelling it is. `min_by_key` on the
        // offset rather than a search per spelling in turn, so that a template
        // holding both kinds splits in the right order.
        let next = WILDCARDS
            .iter()
            .filter_map(|token| rest.find(token).map(|at| (at, token.len())))
            .min_by_key(|(at, _)| *at);
        let Some((at, len)) = next else {
            if !rest.is_empty() {
                parts.push(Part::Literal(rest));
            }
            return parts;
        };
        if at > 0 {
            parts.push(Part::Literal(&rest[..at]));
        }
        parts.push(Part::Wildcard);
        rest = &rest[at + len..];
    }
}

/// One literal run as a regular expression: escaped, with every run of
/// whitespace made flexible.
///
/// `\s+` and not a literal space, for two reasons the spec measures. The
/// ground truth disagrees with its own templates about spacing — Linux `E18`
/// has two spaces before `user=root` where the template has one, and the Mac
/// templates with an embedded newline appear with a space in the structured
/// file. And the literal-space rule is what costs loghub's own matcher 17
/// points on Hadoop.
fn literal_to_pattern(literal: &str) -> String {
    let mut out = String::new();
    let mut rest = literal;
    while !rest.is_empty() {
        let Some(at) = rest.find(char::is_whitespace) else {
            out.push_str(&regex::escape(rest));
            break;
        };
        out.push_str(&regex::escape(&rest[..at]));
        out.push_str(r"\s+");
        rest = rest[at..].trim_start();
    }
    out
}

/// One `EventTemplate` as a recon pattern, or `None` when it would match
/// every line.
///
/// The leading and trailing `.*?` are stripped, and only those two. recon
/// searches an unanchored whole line, so a wildcard at either end constrains
/// nothing and only gives the engine work to do. The whitespace beside it is
/// kept, which is the point of stripping the wildcard rather than trimming the
/// template: `<*> pid = <*>` becomes `\s+pid\s+=\s+`, which still says that
/// `pid` stands alone, where trimming the template first would give `pid`,
/// which also matches `rpid`.
fn pattern_for(template: &str) -> Option<String> {
    // A template with no literal text cannot make a filter, whatever the
    // wildcards do. Tested here, on the template, rather than on the finished
    // pattern: `<*> <*>` renders to `\s+`, and "matches any line with a space"
    // is not something to write into a file.
    let bare = WILDCARDS
        .iter()
        .fold(template.to_string(), |text, token| text.replace(token, ""));
    if bare.trim().is_empty() {
        return None;
    }

    let mut pattern = String::new();
    for part in split_template(template) {
        match part {
            Part::Literal(literal) => pattern.push_str(&literal_to_pattern(literal)),
            Part::Wildcard => pattern.push_str(WILDCARD_PATTERN),
        }
    }
    let pattern = pattern
        .strip_prefix(WILDCARD_PATTERN)
        .unwrap_or(&pattern)
        .to_string();
    let pattern = pattern
        .strip_suffix(WILDCARD_PATTERN)
        .unwrap_or(&pattern)
        .to_string();
    if pattern.is_empty() {
        return None;
    }
    Some(pattern)
}

/// The filter's name in the pane: the template, with every run of whitespace
/// made one space.
///
/// The template itself rather than the `EventId`, because `E323` says nothing
/// on a pane row — and because the spec is explicit that an `EventId` is local
/// to one file and not a stable key. The whitespace pass is what handles the
/// two Mac templates with an embedded newline, which would otherwise put a
/// `\n` escape into a name that has to fit on one row.
fn name_for(template: &str) -> String {
    template.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every template, converted, deduplicated and named.
fn convert(templates: &[String]) -> Outcome {
    let mut outcome = Outcome::default();
    let mut seen_patterns: Vec<String> = Vec::new();
    for template in templates {
        let Some(pattern) = pattern_for(template) else {
            outcome.empty += 1;
            continue;
        };
        // Two templates that render to one pattern are one filter. Dropping
        // the second is not only tidiness: `filters.toml` refuses two filters
        // in a set with the same name, and identical patterns very often carry
        // identical names.
        if seen_patterns.contains(&pattern) {
            outcome.duplicate += 1;
            continue;
        }
        seen_patterns.push(pattern.clone());

        // Distinct patterns can still collide on a name — `<*>` and `<NUM>`
        // in the same position render the same name from different templates.
        // A suffix rather than a refusal: the name is a label, and losing the
        // filter would be the worse trade.
        let base = name_for(template);
        let mut name = base.clone();
        let mut nth = 2;
        while outcome.filters.iter().any(|(seen, _)| *seen == name) {
            name = format!("{base} #{nth}");
            nth += 1;
        }
        outcome.filters.push((name, pattern));
    }
    outcome
}

// ---- writing the file ---------------------------------------------------

/// The set as `filters.toml` text.
///
/// `toml_edit` rather than a `format!` loop, for the reason the pattern is
/// literal-quoted below: the escaping of a name that holds a `"`, a `\` or a
/// newline is the library's problem, not this file's.
fn emit(set: &str, filters: &[(String, String)], source: &Source) -> Result<String> {
    use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

    let mut doc = DocumentMut::new();
    let mut sets = Table::new();
    // `[sets]` on its own says nothing; only `[sets.<name>]` should appear.
    sets.set_implicit(true);

    let mut table = Table::new();
    let mut array = ArrayOfTables::new();
    for (name, pattern) in filters {
        let mut filter = Table::new();
        filter.insert("name", value(name.as_str()));
        filter.insert("pattern", literal_string(pattern)?);
        array.push(filter);
    }
    table.insert("filters", Item::ArrayOfTables(array));
    sets.insert(set, Item::Table(table));
    doc.insert("sets", Item::Table(sets));

    Ok(format!("{}{doc}", header(set, filters.len(), source)))
}

/// The comment block above the set.
///
/// Prepended to the rendered document rather than set as the root table's
/// decor: `toml_edit` drops a prefix on an implicit table, and the header has
/// to survive. It says three things, because a converted set is read months
/// after it is made: where it came from, what to do with it, and which
/// one-line edits the tool deliberately did not make.
fn header(set: &str, count: usize, source: &Source) -> String {
    let origin = source.describe();
    format!(
        r#"# recon filter set, converted from a loghub event-template CSV by
# `loghub2recon`. Source:
#   {origin}
# {count} filters.
#
# Append this to ~/.config/recon/filters.toml. recon never writes that file
# for you.
#
# The set starts disabled, and every filter in it starts off. Enable the
# `{set}` row in recon's filter pane, then turn on the few filters that
# describe what you are looking for. To change that, add by hand:
#
#   [sets.{set}]
#   autoload = true            # start the set enabled
#   priority = 10              # lower is nearer the top; the default is 50
#   [sets.{set}.profiles]
#   default = ["<a filter's name>", ...]   # applied when the set is enabled
#
# loghub's data is licensed for research and academic use, and asks for a
# citation of https://github.com/logpai/loghub.

"#
    )
}

/// `pattern` as a TOML string, single-quoted where TOML allows it.
///
/// The same trick, and for the same reason, as recon's own
/// `filtersets::literal_string`: a literal string does no escape processing,
/// so a regex goes in verbatim with no `\\` tax. `toml_edit` exposes no way to
/// choose a value's quoting, so the literal form is made by parsing one and
/// moving the value across.
fn literal_string(pattern: &str) -> Result<toml_edit::Item> {
    use toml_edit::{DocumentMut, value};

    if pattern.contains('\'') || pattern.contains('\n') || pattern.contains('\r') {
        return Ok(value(pattern));
    }
    let mut one: DocumentMut = format!("pattern = '{pattern}'\n")
        .parse()
        .map_err(|err: toml_edit::TomlError| eyre!("{pattern:?} will not quote: {err}"))?;
    one.as_table_mut()
        .remove("pattern")
        .ok_or_else(|| eyre!("the snippet defines `pattern`"))
}

// ---- proving recon will load it -----------------------------------------

/// `filters.toml` as far as this tool ever writes it.
///
/// `deny_unknown_fields` throughout, as recon's own schema has it, and here it
/// earns more than it does there: this struct is checked only against text
/// `emit` just produced, so an unknown key means *this tool* wrote something
/// it did not mean to.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSchema {
    sets: std::collections::BTreeMap<String, SetSchema>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SetSchema {
    filters: Vec<FilterSchema>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FilterSchema {
    name: String,
    pattern: String,
}

/// Every way recon can refuse a file this tool wrote.
///
/// **Checked against recon 0.1.0**, whose loader is `src/filtersets.rs`,
/// `parse`. Of the rules there, these four are the ones an emitted file can
/// break; the rest — a `colour` that will not parse, a `sense` recon does not
/// know, `mode` set to anything but `"or"`, a `profiles` entry naming a filter
/// that is not there, a built-in set's table carrying filters — are all
/// unreachable, because `emit` never writes those keys and `main` refuses a
/// built-in name before it gets here.
///
/// It runs before the file is written, so a bug here costs the user nothing
/// but a message. That is the whole point of it: the failure this guards
/// against is not one the user could diagnose at their next recon start.
fn validate(text: &str, set: &str) -> Result<()> {
    let bug = |message: String| {
        eyre!("loghub2recon produced a file recon will not load; this is a bug: {message}")
    };

    // 1. It is TOML, and it is the shape recon's schema describes.
    let file: FileSchema = toml::from_str(text).map_err(|err| bug(err.to_string()))?;
    let Some(schema) = file.sets.get(set) else {
        return Err(bug(format!("no [sets.{set}] table")));
    };
    // 2. A set's name cannot be empty, and a file set needs a filter.
    if set.is_empty() {
        return Err(bug("the set's name is empty".into()));
    }
    if schema.filters.is_empty() {
        return Err(bug(format!("[sets.{set}] has no filters")));
    }

    let mut seen: Vec<&str> = Vec::with_capacity(schema.filters.len());
    for filter in &schema.filters {
        // 3. Every pattern compiles, with the same engine recon uses. This is
        //    the check that matters: `regex::escape` is what stands between a
        //    template's `(`, `*` or `\` and a file that will not load.
        regex::Regex::new(&filter.pattern).map_err(|err| {
            bug(format!(
                "filter {:?} has a pattern recon cannot compile: {err}",
                filter.name
            ))
        })?;
        // 4. No two filters in one set share a name.
        if seen.contains(&filter.name.as_str()) {
            return Err(bug(format!("two filters named {:?}", filter.name)));
        }
        seen.push(&filter.name);
    }
    Ok(())
}

// ---- what the user is told ----------------------------------------------

/// The summary, on stderr so that `-o -` stays a clean pipe.
fn report(outcome: &Outcome, set: &str, target: &str) {
    let count = outcome.filters.len();
    eprintln!("[sets.{set}]: {count} filters");
    if outcome.empty > 0 {
        eprintln!(
            "  {} template(s) skipped: nothing but wildcards, so the pattern \
             would match every line",
            outcome.empty
        );
    }
    if outcome.duplicate > 0 {
        eprintln!(
            "  {} template(s) skipped: another template gave the same pattern",
            outcome.duplicate
        );
    }
    if count > NAVIGATOR_CEILING {
        eprintln!(
            "  note: recon's navigator stops matching above {NAVIGATOR_CEILING} known \
             patterns across all sets. This set alone is over it, so keep it disabled \
             unless you are using it."
        );
    }
    if target != "-" {
        eprintln!("  append {target} to ~/.config/recon/filters.toml to use it");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An emitted file read back as `(name, pattern)` pairs, in file order.
    /// Through `toml`, so the assertion is about what a TOML reader sees and
    /// not about the quoting `toml_edit` happened to choose.
    fn read_back(text: &str, set: &str) -> Vec<(String, String)> {
        let file: FileSchema = toml::from_str(text).expect("valid TOML");
        file.sets[set]
            .filters
            .iter()
            .map(|filter| (filter.name.clone(), filter.pattern.clone()))
            .collect()
    }

    /// The worked examples from the spec, end to end. Each is a real template
    /// from a real dataset, with the raw line it has to match.
    #[test]
    fn the_specs_worked_examples_convert_and_match() {
        let cases = [
            (
                "Thermal pressure state: <*> Memory pressure state: <*>",
                "Jul  1 09:01:05 host com.apple.CDScheduler[43]: Thermal pressure state: 1 \
                 Memory pressure state: 0",
            ),
            (
                "PacketResponder <*> for block <*> terminating",
                "081109 203615 148 INFO dfs.DataNode$PacketResponder: PacketResponder 1 for \
                 block blk_38865049064139660 terminating",
            ),
            (
                "check pass; user unknown",
                "Jun 14 15:16:02 combo sshd(pam_unix)[19937]: check pass; user unknown",
            ),
            (
                "mod_jk child workerEnv in error state <*>",
                "[Sun Dec 04 04:47:44 2005] [error] mod_jk child workerEnv in error state 6",
            ),
            (
                "acquire lock=<*>, flags=<*>, tag=\"<*>\", name=<*>, ws=<*>, uid=<*>, pid=<*>",
                "03-17 16:13:38.819  1702  8671 D PowerManagerService: acquire lock=233570404, \
                 flags=0x1, tag=\"View Lock\", name=com.android.systemui, ws=null, uid=10037, \
                 pid=2227",
            ),
            (
                "<*>(<*>)::listenerCallback - Thunderbolt HPD packet for route = <*> port = \
                 <*> unplug = <*>",
                "Jul  1 09:00:55 host kernel[0]: IOThunderboltSwitch<0>(0x0)::listenerCallback \
                 - Thunderbolt HPD packet for route = 0x0 port = 11 unplug = 0",
            ),
        ];
        for (template, line) in cases {
            let pattern = pattern_for(template).expect("a usable pattern");
            let regex = regex::Regex::new(&pattern).expect("compiles");
            assert!(regex.is_match(line), "{template:?} -> {pattern:?}");
        }
    }

    /// No `^` and no `$`: the template describes the message body, and recon
    /// matches the whole raw line, header and all.
    #[test]
    fn patterns_are_not_anchored() {
        let pattern = pattern_for("check pass; user unknown").expect("usable");
        assert_eq!(pattern, "check\\s+pass;\\s+user\\s+unknown");
    }

    /// Every character that is not a wildcard is literal, `*` included. 23
    /// templates hold a bare `*` and five of them are in the spec by name.
    #[test]
    fn literal_metacharacters_are_escaped() {
        let cases = [
            (
                "BLOCK* ask <*> to delete <*>",
                "BLOCK* ask 10.251.73.220:50010 to delete blk_-7778709377942981985",
            ),
            (
                "******* GOODBYE <*>:<*> ********",
                "******* GOODBYE /10.10.34.11:57854 ********",
            ),
            ("*** info [mice.c(<*>)]:", "*** info [mice.c(174)]:"),
            (
                "(DiskStore.Normal:<*>) <*>",
                "(DiskStore.Normal:2) something",
            ),
            ("$QuotingInputFilter <*>", "$QuotingInputFilter 3"),
        ];
        for (template, line) in cases {
            let pattern = pattern_for(template).expect("usable");
            let regex = regex::Regex::new(&pattern).expect("compiles");
            assert!(regex.is_match(line), "{template:?} -> {pattern:?}");
        }
    }

    /// The split is on the exact three-character token. Literal angle brackets
    /// survive — `<ok>`, `<<<< Boss >>>>` and the rest of the spec's list.
    #[test]
    fn literal_angle_brackets_survive() {
        for template in [
            "<ok>",
            "<ABORT code completed>",
            "<<<< Boss >>>>",
            "< FigByteStream >",
        ] {
            let pattern = pattern_for(template).expect("usable");
            let regex = regex::Regex::new(&pattern).expect("compiles");
            assert!(regex.is_match(template), "{template:?} -> {pattern:?}");
        }
    }

    /// `<NUM>` is the wildcard under another name, and a template may hold
    /// both spellings.
    #[test]
    fn num_is_a_wildcard_too() {
        let template = "CCFile::captureLog Received Capture notice id: <*>, reason = \
                        RoamFail:sts:<NUM>_rsn:<NUM>";
        let pattern = pattern_for(template).expect("usable");
        let regex = regex::Regex::new(&pattern).expect("compiles");
        assert!(regex.is_match(
            "CCFile::captureLog Received Capture notice id: 4, reason = RoamFail:sts:8_rsn:2"
        ));
    }

    /// Whitespace is flexible in both directions: the template's single space
    /// matches the ground truth's two, and a template with an embedded newline
    /// matches the line that has a space in its place.
    #[test]
    fn whitespace_is_flexible() {
        let template = "authentication failure; logname= uid=<*> euid=<*> tty=<*> ruser= \
                        rhost=<*> user=<*>";
        let pattern = pattern_for(template).expect("usable");
        let regex = regex::Regex::new(&pattern).expect("compiles");
        assert!(
            regex.is_match(
                "authentication failure; logname= uid=0 euid=0 tty=NODEVssh ruser= \
                 rhost=220-135-151-1.hinet-ip.hinet.net  user=root"
            ),
            "two spaces before user=root: {pattern:?}"
        );

        let newline = "Arranged view frame: {{<*>, <*>}\n{<*>, <*>}}";
        let pattern = pattern_for(newline).expect("usable");
        assert!(!pattern.contains('\n'), "no raw newline: {pattern:?}");
        let regex = regex::Regex::new(&pattern).expect("compiles");
        assert!(regex.is_match("Arranged view frame: {{0, 0} {1024, 768}}"));
    }

    /// A wildcard at either end is stripped, and the whitespace beside it is
    /// not. Stripping the template instead would lose the word boundary.
    #[test]
    fn edge_wildcards_are_stripped_but_their_spacing_is_kept() {
        assert_eq!(pattern_for("<*> pid = <*>"), Some(r"\s+pid\s+=\s+".into()));
        assert_eq!(
            pattern_for("<*>-_continuousScroll is deprecated"),
            Some(r"\-_continuousScroll\s+is\s+deprecated".into())
        );
        // Only the outermost one goes; an interior wildcard is a real gap.
        let pattern = pattern_for("<*> a <*> b <*>").expect("usable");
        assert_eq!(pattern, r"\s+a\s+.*?\s+b\s+");
    }

    /// A template with no literal text is not a filter. `<*>` alone would
    /// become `.*?`, and `<*> <*>` would become `\s+`.
    #[test]
    fn an_all_wildcard_template_is_refused() {
        for template in ["<*>", "<*> <*>", "<*><*>", "<NUM> <*>", "   "] {
            assert_eq!(pattern_for(template), None, "{template:?}");
        }
    }

    #[test]
    fn names_are_the_template_on_one_line() {
        assert_eq!(name_for("a  b\nc"), "a b c");
        assert_eq!(
            name_for("Thermal pressure state: <*>"),
            "Thermal pressure state: <*>"
        );
    }

    /// Two templates that render to one pattern are one filter. Spacing and
    /// the spelling of the wildcard are both things the pattern forgets, so
    /// three of these four rows are the same filter.
    #[test]
    fn templates_that_render_alike_become_one_filter() {
        let outcome = convert(&[
            "size <*>".to_string(),
            "size  <*>".to_string(),  // \s+ eats the difference
            "size <NUM>".to_string(), // the other spelling of the wildcard
            "<*>".to_string(),        // no literal text at all
            "free <*>".to_string(),
        ]);
        assert_eq!(outcome.duplicate, 2);
        assert_eq!(outcome.empty, 1);
        let names: Vec<&str> = outcome
            .filters
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["size <*>", "free <*>"], "the first row wins");
    }

    /// Distinct patterns can still collide on a name, because a name is the
    /// template with its whitespace flattened. The second takes a suffix
    /// rather than being dropped: `filters.toml` refuses two filters in one
    /// set with the same name, and losing a filter is the worse trade.
    #[test]
    fn a_name_collision_takes_a_suffix() {
        let outcome = convert(&[" size <*>".to_string(), "size <*>".to_string()]);
        assert_eq!(outcome.duplicate, 0, "\\s+size\\s+ is not size\\s+");
        let names: Vec<&str> = outcome
            .filters
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["size <*>", "size <*> #2"]);
    }

    /// The header row is found by name, an embedded newline does not shift a
    /// row, and an extra column is ignored.
    #[test]
    fn the_csv_is_read_as_csv() {
        let csv = "EventId,EventTemplate,Occurrences\n\
                   E1,size <*>,3\n\
                   E2,\"Arranged view frame: {{<*>, <*>}\n{<*>, <*>}}\",1\n\
                   E3,,0\n\
                   E4,\"connection from \"\"#<*>#\"\"\",7\n";
        let templates = read_templates(csv, "t").expect("read");
        assert_eq!(templates.len(), 3, "the empty cell is not a template");
        assert!(templates[1].contains('\n'), "one row, not two");
        assert_eq!(templates[2], "connection from \"#<*>#\"");

        let message = read_templates("Id,Body\nE1,x\n", "t")
            .expect_err("no EventTemplate column")
            .to_string();
        assert!(message.contains("EventTemplate"), "{message}");
    }

    /// The whole point: what this tool writes, recon loads. Asserted against
    /// the very parser `filtersets::load_file` uses.
    #[test]
    fn the_emitted_file_is_one_recon_loads() {
        let templates = [
            "Thermal pressure state: <*> Memory pressure state: <*>".to_string(),
            "check pass; user unknown".to_string(),
            "it's <*> quoted".to_string(),
            "BLOCK* ask <*> to delete <*>".to_string(),
        ];
        let outcome = convert(&templates);
        let text = emit("Mac", &outcome.filters, &Source::Dataset("Mac")).expect("emits");

        assert!(text.starts_with("# recon filter set"), "{text}");
        assert!(text.contains("raw.githubusercontent.com"), "{text}");
        assert!(text.contains("[sets.Mac]"), "{text}");
        assert!(!text.contains("\n[sets]\n"), "no bare [sets]: {text}");
        assert!(
            !text.contains("\nautoload") && !text.contains("\npriority"),
            "shown in the header comment, never set:\n{text}"
        );
        assert!(
            text.contains(r"pattern = 'BLOCK\*\s+ask\s+.*?\s+to\s+delete\s+'"),
            "a literal string, unescaped:\n{text}"
        );
        assert!(
            !text.contains(r"\\s+"),
            "no backslash tax anywhere in the file:\n{text}"
        );

        validate(&text, "Mac").expect("recon would load it");
        let mac = read_back(&text, "Mac");
        assert_eq!(mac.len(), 4);
        assert_eq!(mac[1].0, "check pass; user unknown");
        assert_eq!(
            mac[2].1, r"it's\s+.*?\s+quoted",
            "a pattern holding a quote survives the round trip"
        );
        assert!(
            !text.contains("\n[sets.Mac.profiles]"),
            "no profile the user did not ask for; the header comment shows \
             where one would go:\n{text}"
        );
    }

    /// The awkward corners of quoting, each proved by the round trip rather
    /// than by the spelling `toml_edit` chose. A pattern is written as a
    /// literal string so that no regex ever pays a `\\` tax; which *kind* of
    /// literal is the library's decision, and `validate` proves the result
    /// either way.
    #[test]
    fn awkward_patterns_still_round_trip() {
        let templates = [
            "won't <*>".to_string(),
            "ends with a quote <*>'".to_string(),
            "'quoted'".to_string(),
            r"back\slash <*>".to_string(),
            "tab\tseparated <*>".to_string(),
        ];
        let outcome = convert(&templates);
        assert_eq!(outcome.filters.len(), templates.len());
        let text = emit(
            "q",
            &outcome.filters,
            &Source::Local(PathBuf::from("t.csv")),
        )
        .expect("emits");
        validate(&text, "q").expect("recon would load it");
        assert_eq!(read_back(&text, "q"), outcome.filters);
    }

    #[test]
    fn a_dataset_name_resolves_without_regard_to_case() {
        let source = Source::resolve("openssh").expect("a dataset");
        assert!(matches!(source, Source::Dataset("OpenSSH")));
        assert_eq!(source.default_set_name(), "OpenSSH");
        assert!(source.describe().ends_with("OpenSSH_2k.log_templates.csv"));
    }

    /// A mistyped path is refused as a path. Falling through to the dataset
    /// branch would answer a path question with a list of 16 log types.
    #[test]
    fn a_missing_path_is_refused_as_a_path() {
        let message = Source::resolve("./no/such/file.csv")
            .expect_err("missing")
            .to_string();
        assert!(message.contains("no such file"), "{message}");

        let message = Source::resolve("Macintosh")
            .expect_err("unknown")
            .to_string();
        assert!(
            message.contains("neither a file nor a loghub log type"),
            "{message}"
        );
        assert!(message.contains("OpenSSH"), "lists the types: {message}");
    }

    #[test]
    fn a_file_name_gives_up_its_set_name() {
        let name = |file: &str| Source::Local(PathBuf::from(file)).default_set_name();
        assert_eq!(name("Mac_2k.log_templates.csv"), "Mac");
        assert_eq!(name("/tmp/Mac_2k.log_templates_corrected.csv"), "Mac");
        assert_eq!(name("wifi.csv"), "wifi");
        assert_eq!(name("plain"), "plain");
    }

    // ---- `validate`, which stands in for recon's own loader --------------

    /// Each of the four rules, refused. Written against hand-made text rather
    /// than against `emit`'s output, because the whole job of these is to fail
    /// — and `emit` is not supposed to be able to produce any of them.
    #[test]
    fn validate_refuses_what_recon_would_refuse() {
        let refused = |text: &str, set: &str| {
            validate(text, set)
                .expect_err("recon would refuse this")
                .to_string()
        };

        // 1. Not TOML at all, and a key recon's schema does not define.
        assert!(refused("[sets.a", "a").contains("will not load"));
        let unknown = refused(
            "[[sets.a.filters]]\nname = 'x'\npattern = 'y'\ncolor = 'red'\n",
            "a",
        );
        assert!(unknown.contains("color"), "{unknown}");

        // 2. A set with no filters, and a set that is not there.
        assert!(refused("[sets.a]\nfilters = []\n", "a").contains("no filters"));
        assert!(refused("[sets.a]\nfilters = []\n", "b").contains("no [sets.b] table"));

        // 3. A pattern recon cannot compile. This is the rule that earns the
        //    function: `(` is a literal in 40-odd loghub templates.
        let bad = refused("[[sets.a.filters]]\nname = 'opener'\npattern = '('\n", "a");
        assert!(bad.contains("cannot compile"), "{bad}");
        assert!(bad.contains("opener"), "names the filter: {bad}");

        // 4. Two filters in one set with the same name.
        let twice = refused(
            "[[sets.a.filters]]\nname = 'x'\npattern = 'p'\n\
             [[sets.a.filters]]\nname = 'x'\npattern = 'q'\n",
            "a",
        );
        assert!(twice.contains("two filters named \"x\""), "{twice}");
    }

    /// Every failure `validate` reports is this tool's, not the user's, and
    /// says so — a message that read like a user error would send someone to
    /// edit a file they never wrote.
    #[test]
    fn a_validate_failure_calls_itself_a_bug() {
        let message = validate("[[sets.a.filters]]\nname = 'x'\npattern = '('\n", "a")
            .expect_err("refused")
            .to_string();
        assert!(message.contains("loghub2recon produced"), "{message}");
        assert!(message.contains("this is a bug"), "{message}");
    }

    /// Whatever `convert` makes of the real datasets, `validate` accepts. The
    /// templates here are the ones from the spec's own list of things that
    /// break a naive converter.
    #[test]
    fn every_awkward_template_passes_validation() {
        let templates: Vec<String> = [
            "############################## _getSysMsgList",
            "(DiskStore.Normal:<*>) <*> <*>",
            "BLOCK* ask <*> to delete <*>",
            "******* GOODBYE <*>:<*> ********",
            "*** info [mice.c(<*>)]:",
            "<<<< Boss >>>>",
            "< expand ICONS* >",
            "$QuotingInputFilter <*>",
            "a|b <*> c+d <*> e?f",
            r"path\to\<*>",
            "Arranged view frame: {{<*>, <*>}\n{<*>, <*>}}",
            "CCFile::captureLog id: <*>, RoamFail:sts:<NUM>_rsn:<NUM>",
        ]
        .iter()
        .map(|t| (*t).to_string())
        .collect();
        let outcome = convert(&templates);
        assert_eq!(outcome.filters.len(), templates.len(), "none dropped");
        let text = emit("awkward", &outcome.filters, &Source::Dataset("Mac")).expect("emits");
        validate(&text, "awkward").expect("recon would load it");
        assert_eq!(read_back(&text, "awkward"), outcome.filters);
    }

    /// `definitions` is recon's, and a table under it may carry no filters.
    /// The refusal is in `main`, above `emit`; this pins the list it reads so
    /// that a second built-in name is not added in one place only.
    #[test]
    fn the_builtin_set_names_are_known() {
        assert!(BUILTIN_SETS.contains(&"definitions"));
        assert!(!BUILTIN_SETS.contains(&"Mac"));
    }
}
