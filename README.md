# loghub2recon

Convert a [loghub](https://github.com/logpai/loghub) event-template CSV into a
filter set for [recon](https://github.com/PeteRichardson/recon).

loghub publishes 2000-line samples of 16 log formats — Android, Mac, OpenSSH,
Windows and twelve more — and, beside each one, a CSV of the event templates a
log parser found in it. A template is a mask of one message:

```
Thermal pressure state: <*> Memory pressure state: <*>
```

`<*>` marks the position that held instance-specific data. That is very nearly
a recon filter. This tool does the last step.

## Install

```sh
cargo install --path .
```

Or build in place with `cargo build --release`; the binary is
`target/release/loghub2recon`.

Rust stable. Naming a log type needs `curl` on the PATH; converting a local CSV
does not.

## Use

```sh
loghub2recon --list            # the 16 log types you may name
loghub2recon OpenSSH           # fetches the CSV, writes ./OpenSSH-filters.toml
loghub2recon Mac.csv           # or convert a CSV you already have
loghub2recon Mac -o -          # or write to stdout
```

| Flag | Meaning |
| --- | --- |
| `--list` | Print the log types that may be named instead of a path, and exit |
| `--set-name NAME` | The set's name in the emitted file. Defaults to the log type, or the leading part of the CSV's file name |
| `-o`, `--output PATH` | Where to write. `-` is stdout. Defaults to `<set>-filters.toml` |
| `--force` | Overwrite the output file if it is already there |

Naming a log type fetches
`https://raw.githubusercontent.com/logpai/loghub/master/<Name>/<Name>_2k.log_templates.csv`.
Pass a path instead if the machine has no network.

## What you get

A `filters.toml` fragment with one filter per template. Append it to
`~/.config/recon/filters.toml`:

```toml
[sets.OpenSSH]

[[sets.OpenSSH.filters]]
name    = "Failed password for <*> from <*> port <*> ssh2"
pattern = 'Failed\s+password\s+for\s+.*?\s+from\s+.*?\s+port\s+.*?\s+ssh2'
```

The emitted file carries a header comment saying where it came from and what
to do with it, so a set found months later explains itself.

## Three things about the patterns

- **They are not anchored.** loghub's own matcher anchors with `^` and `$`,
  because it matches a message body that the log format has already had its
  timestamp, host, component and PID cut from. recon matches whole raw lines
  with `Regex::is_match`, so an anchored pattern would never fire. A wildcard
  at either end is dropped for the same reason — it constrains nothing in an
  unanchored search — but the whitespace beside it is kept, so `<*> pid = <*>`
  becomes `\s+pid\s+=\s+` and does not also match `rpid`.
- **Every run of whitespace becomes `\s+`.** The sample logs disagree with
  their own templates about spacing. Linux `E18` has two spaces where the
  template has one; two Mac templates hold an embedded newline where the
  structured ground truth has a space. A literal space costs loghub's own
  matcher 17 points on Hadoop.
- **The set starts disabled and every filter in it starts off.** A dataset can
  be 341 templates, which is not a thing to switch on at once — and well past
  recon's 64-pattern navigator ceiling. Enable the set, then turn on the
  handful that describe what you are chasing. `autoload`, `priority` and a
  `default` profile are one-line hand edits, and the emitted file shows where
  each one goes.

## Measured

Converted against the live loghub repository, then matched back against those
datasets' raw `_2k.log` files:

| Dataset | Filters | Lines matched |
| --- | --- | --- |
| Apache | 6 | 2000 / 2000 |
| HDFS | 14 | 2000 / 2000 |
| OpenSSH | 27 | 2000 / 2000 |
| Windows | 50 | 2000 / 2000 |
| Android | 166 | 2000 / 2000 |
| Thunderbird | 149 | 2000 / 2000 |
| Mac | 341 | 2000 / 2000 |

## How this relates to recon

This tool lived inside the recon repository until it was split out. There it
validated its output by calling recon's own loader — the function recon reads
`filters.toml` with — which was the best thing about it.

A dependency edge back to recon would not pay for itself: recon's loader
reaches into its colour parser and its syntax kinds, so a git dependency would
compile ratatui, crossterm, syntect and two-face in order to convert a CSV.
`validate` in `src/main.rs` is the stand-in, and it is equivalent *for what
this tool emits* — every rule it does not check is one an emitted file cannot
break. It records which recon version that was true of.

## Documentation

[`docs/loghub-template-format.md`](docs/loghub-template-format.md) is the
reference: how to read a loghub template file, what is surprising about it,
which rows in the shipped data are faulty, and how the conversion recipe was
measured.

## Licence

MIT — see [LICENSE](LICENSE).

loghub's own data is licensed for research and academic use and asks for a
citation of the loghub repository. That applies to a converted filter set too.
