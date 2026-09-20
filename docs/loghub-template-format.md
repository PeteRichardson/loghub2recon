# The loghub event template format

How to read `*_templates_corrected.csv`, and how to make usable regular
expressions from it.

Everything below was measured against the [logparser][] repository and the
`logparser` package it publishes on PyPI. Two shorthands are used throughout:

| Shorthand | What it is |
|---|---|
| `$LOGPARSER` | a clone of <https://github.com/logpai/logparser> |
| `$SITE_PACKAGES` | where `pip install logparser` put the package — `python -c "import logparser, os; print(os.path.dirname(logparser.__file__))"` |

The two hold **different code**. The published package lags the repository:
section 7, item 3 is a fault that `$LOGPARSER` repairs and `$SITE_PACKAGES`
still has. Line numbers below are the published package's unless the path says
`$LOGPARSER`.

Data: `$LOGPARSER/data/loghub_2k_corrected/` — 16 datasets, 1347 templates.

[logparser]: https://github.com/logpai/logparser

---

## 1. What the file is

Two columns. Standard RFC4180 CSV:

```
EventId,EventTemplate
E1,############################## _getSysMsgList
E2,(DiskStore.Normal:<*>) <*> <*>
E3,(ImportBailout.Error:<*>) Asked to exit for Diskarb
```

- `EventId` is a label (`E1`...`En`). It is local to one file. It is **not** a
  stable key (see section 5).
- `EventTemplate` is a mask of one log message **body**. A log parser (Drain,
  Spell, and others) made the mask. The authors of the ICSE'22 paper then
  corrected it by hand.
- `<*>` marks a position that held instance-specific data.
- All other text is **literal**. This includes the characters `*`, `.`, `(`,
  `[`, `+`, `|`, `?`, `$` and `\`.

A template is not a regular expression. It is a token mask. You must convert
it.

### Templates hold no header

A template matches only the message body. `LogLoader` removes the timestamp,
host, component and PID first (`$SITE_PACKAGES/utils/logloader.py:72-86`). It
uses a `log_format` string, one for each dataset. It makes each `<Field>` into
a lazy named group, and it makes each run of spaces into `\s+`.

Example, Mac:

```
raw      Jul  1 09:01:05 calvisitor-10-105-160-95 com.apple.CDScheduler[43]: Thermal pressure state: 1 Memory pressure state: 0
format   <Month>  <Date> <Time> <User> <Component>\[<PID>\]( \(<Address>\))?: <Content>
Content  Thermal pressure state: 1 Memory pressure state: 0
template Thermal pressure state: <*> Memory pressure state: <*>      (E323)
```

`LogLoader` also does two more things to each line: it removes leading and
trailing space, and it replaces each run of non-ASCII bytes with the literal
text `<N/ASCII>` (`logloader.py:95`). A line that the format does not fit is
discarded with a `Skip line:` message (`logloader.py:102`). This moves the
`LineId` values out of alignment.

---

## 2. How logparser makes a regular expression

One function does all of it — `RegexMatch._generate_template_regex`, at
`$SITE_PACKAGES/logmatch/regexmatch.py:63-70`:

```python
template = re.sub(r"(<\*>\s?){2,}", "<*>", template)   # collapse wildcard runs
regex = re.sub(r"([^A-Za-z0-9])", r"\\\1", template)   # escape all non-alphanumerics
regex = regex.replace("\<\*\>", "(.*?)")
regex = regex.replace("\<NUM\>", "(([\-|\+]?\d+)|(0[Xx][a-fA-F\d]+))")
regex = regex.replace("\<IP\>", "((\d+\.){3}\d+)")
regex = "^" + regex + "$"
```

Points to note:

- The escape step is a sledgehammer. It puts a backslash before **every**
  character that is not `A-Za-z0-9`. This is also how `<*>` becomes `\<\*\>`,
  which the next line then finds.
- `<*>` becomes `(.*?)` — a capture group that is not greedy.
- The result is anchored with `^` and `$`.
- The module imports the third-party `regex` package, not `re`
  (`regexmatch.py:23`).
- `<IP>` does not occur in any loghub file. Ignore it.

Results, confirmed by a run of the function:

| template | regular expression |
|---|---|
| `############################## _getSysMsgList` | `^\#\#…\#\ \_getSysMsgList$` |
| `(DiskStore.Normal:<*>) <*> <*>` | `^\(DiskStore\.Normal\:(.*?)\)\ (.*?)$` |
| `Connection from <*> port <*> on <*> port <*>` | `^Connection\ from\ (.*?)\ port\ (.*?)\ on\ (.*?)\ port\ (.*?)$` |

Look at row 2. Three wildcards became two groups. Section 3 tells you why.

---

## 3. Behavior that surprises you

### Runs of wildcards collapse, and the space between them disappears

Line 64 replaces two or more adjacent `<*>` with one `<*>`. The `\s?` in the
pattern is greedy, so it also eats the space:

```
(DiskStore.Normal:<*>) <*> <*>   ->  ^\(DiskStore\.Normal\:(.*?)\)\ (.*?)$
Found <*> <*> items              ->  ^Found\ (.*?)items$
```

**The count of `<*>` in the CSV is not the count of capture groups.** Do not
use one to index the other.

### Space is literal in the matcher

A space becomes `\ `, which matches one space and nothing else. Two spaces in a
template match only two spaces.

The ground truth breaks this rule. The real content for Linux `E18` is:

```
authentication failure; logname= uid=0 euid=0 tty=NODEVssh ruser= rhost=220-135-151-1.hinet-ip.hinet.net  user=root
```

Note the two spaces before `user=root`. The template has one space. The
logparser matcher misses this line.

### A different helper does normalize space

`Drain.get_parameter_list` (`$SITE_PACKAGES/Drain/Drain.py:362`) builds a
second, better regular expression:

```python
template_regex = re.sub(r"<.{1,5}>", "<*>", row["EventTemplate"])
template_regex = re.sub(r"([^A-Za-z0-9])", r"\\\1", template_regex)
template_regex = re.sub(r"\\ +", r"\\s+", template_regex)     # <- normalizes space
template_regex = "^" + template_regex.replace("\<\*\>", "(.*?)") + "$"
```

It normalizes space, and it does **not** collapse wildcard runs. Identical
copies are at `Spell.py:355`, `AEL.py:249` and `IPLoM.py:725`. The equivalent
method on `RegexMatch` (`regexmatch.py:150`) is dead code — nothing calls it.

The two functions thus disagree. Copy the `Drain` behavior, not the
`RegexMatch` behavior.

### `<*>` is not the only placeholder

Mac rows `E168` to `E171` use `<NUM>`:

```
E168,"CCFile::captureLog Received Capture notice id: <*>, reason = RoamFail:sts:<NUM>_rsn:<NUM>"
```

`Mac_2k.log_structured_corrected.csv` writes `<*>` for those same four events.
Treat `<NUM>` as a second spelling of the wildcard. `<ID>` and `<IP>` never
occur.

### Some text in angle brackets is literal

| text | where |
|---|---|
| `<ok>` | HPC `E20` |
| `<ABORT code completed>` | HPC `E37` |
| `<<<< Boss >>>>` | Mac `E12` |
| `<<<< MediaValidator >>>>` | Mac `E100` |
| `< CRABS >`, `< IQ-CA >`, `< FigByteStream >` | Mac |
| `< expand ICONS* … >` | Android `E42` |

A substitution on `<[^>]*>` destroys all of these. **Split on the exact
three-character token `<*>`.**

### A bare `*` is literal

23 rows contain one. Examples:

```
HDFS      E4   BLOCK* ask <*> to delete <*>
Zookeeper E1   ******* GOODBYE <*>:<*> ********
Mac       E4   **** [BroadcomBluetoothHostController][SetupController] -- Delay HCI Reset by <*> ****
HPC       E17  Fan speeds ( <*> <*> <*> **** <*> <*> )
Linux     E1   *** info [mice.c(<*>)]:
```

### 321 of 1347 templates have no wildcard

These are constant messages, for example Linux `E27` `check pass; user
unknown`. They are still useful. Make them exact-match filters.

### Two Mac templates hold an embedded newline

`E152` and `E281`. For example:

```
E152 = Arranged view frame: {{<*>, <*>}\n{<*>, <*>}}
```

`Mac_2k.log_templates_corrected.csv` thus shows 343 physical lines but holds
341 rows. This is correct. **Use a real CSV parser, not a line reader.**

The structured file writes a space in place of that newline, so the two files
disagree. The `\s+` rule in section 6 repairs this at no cost.

### Parameters are not in the data

No file in either data directory has a `ParameterList` column. The ground truth
gives you `Content` and `EventTemplate`, and expects you to extract the
parameters yourself.

The tool output is different from the ground truth. Drain writes
`result/unknow.log_structured.csv` **with** a `ParameterList` column, and it
writes a third column `Occurrences` in its templates file. Do not assume that
the two schemas agree.

---

## 4. How the matcher picks between templates

More than one template can match a line. `regex_match` sorts and then takes the
first hit (`regexmatch.py:203-214`):

```python
match_dict = OrderedDict(
    sorted(
        match_dict.items(),
        key=lambda x: (len(x[1][1]), -x[1][1].count("<*>")),
        reverse=True,
    )
)
for regex, event in match_dict.items():
    parameter_list = re.findall(regex, msg.strip())
    if parameter_list:
        matched_event = event
        break
```

The rule: **longest template text first, then fewest `<*>` first.** Row order
in the CSV decides only between templates of equal length and equal wildcard
count.

Note that `regex_match` redoes this sort for **every log line**. In a harvester,
sort one time and keep the order.

A line that nothing matches gets `("NONE", "NONE")`.

---

## 5. Faults in the data

Four things will break a harvester. Design for them.

### Thunderbird: a stray quote loses 5 templates

Line 29 of `Thunderbird_2k.log_templates_corrected.csv`:

```
E28,Compressed image found at block <*>
E29,"connection from <*>            <- this quote has no partner
E30,connection lost: 'Connection closed.'
E31,Creating Topspin <*> entries:
E32,data_thread() got not answer from any [<*>] datasource
E33,Detected <*> MHz processor.
E34,"Device: <*>, Temperature changed <*> Celsius to <*> Celsius since last report"
```

`E29` holds no comma, so the quote is spurious. A conformant CSV parser absorbs
lines 30 to 35 into one field, and `E30` to `E34` disappear. The file has 149
physical lines but gives only 144 rows.

The cost is large: `E32` alone matches 819 of the 2000 Thunderbird lines. The
match rate falls from 100% to 58.7%.

**Repair:** delete that one quote character. Measured result — 149 rows, and
the match rate goes back to 100.0%.

### Linux: EventId is not a safe key

`Linux_2k.log_templates_corrected.csv` has no `E17` and no `E19`. Both became
duplicates of `E18` during the correction, and the authors deleted them. But
`Linux_2k.log_structured_corrected.csv` still gives those IDs to 21 rows.

**Join on the template text, never on `EventId`.**

### The two corrected files disagree

The `EventTemplate` column of `*_structured_corrected.csv` is different from
`*_templates_corrected.csv` for Linux (5 IDs), Mac (6), OpenSSH (1) and
Thunderbird (2). Examples:

```
Linux E13   templates : Authentication failed from <*> <*>: Permission denied in replay cache code
            structured: Authentication failed from <*>: Permission denied in replay cache code
Thunderbird E134 templates : User #<*>#, coming from #<*>#, authenticated.
                 structured: User <*>, coming from <*>, authenticated.
```

`*_templates_corrected.csv` is the authoritative ICSE'22 artifact. Use it.

### The corrected set is not a copy of the original

Apache is the only dataset that is identical between `loghub_2k/` and
`loghub_2k_corrected/`. Template counts changed for Android (166 → 158), Linux
(118 → 116), OpenSSH (27 → 26) and Thunderbird (149 → 144).

Other pandas artifacts to expect: HDFS stores its date as `81109` (the leading
zero is gone), and Linux stores its PID as `19939.0`.

---

## 6. The recommended recipe

```python
import re

WILDCARDS = ("<*>", "<NUM>")


def template_to_regex(template):
    """Make one loghub EventTemplate into a compiled regular expression."""
    parts = re.split(r"(<\*>|<NUM>)", template)
    out, n = [], 0
    for part in parts:
        if part in WILDCARDS:
            n += 1
            out.append("(?P<p%d>.*?)" % n)
        else:
            literal = re.escape(part)
            literal = re.sub(r"(?:\\\s)+", lambda m: r"\s+", literal)
            out.append(literal)
    return re.compile("^" + "".join(out) + "$")
```

Use it like this:

```python
rows = list(csv.DictReader(open(path, newline="", encoding="utf-8")))
templates = sorted(
    (r["EventTemplate"] for r in rows if r.get("EventTemplate")),
    key=lambda t: (len(t), -t.count("<*>")),
    reverse=True,
)
patterns = [(t, template_to_regex(t)) for t in templates]

for template, pattern in patterns:          # sorted one time, reused
    m = pattern.match(content.strip())      # note the strip()
    if m:
        return template, m.groupdict()
```

Why each part is there:

- **`re.split` with a capture group** keeps the delimiters, so you get the
  literal parts and the wildcards in one pass. It splits only on the exact
  tokens, so `<ok>` and `<<<< Boss >>>>` stay intact.
- **`re.escape`** makes each literal part safe, which covers `BLOCK*`, `x+`,
  `a|b`, `$QuotingInputFilter` and the rest.
- **`(?:\\\s)+` → `\s+`** makes one flexible run of whitespace. Two details:
  Python 3.7 and later escape a space as `\ `, so the pattern must look for a
  backslash and then a space, not for a bare space. And the replacement must be
  a `lambda`, because the plain string `r"\s+"` is an invalid escape in a
  replacement from Python 3.12.
- **Named groups `p1`, `p2`, ...** give parameters straight from
  `m.groupdict()`. This also avoids the `findall` fault in section 7.
- **`.*?`, not `.+?`** — a wildcard can capture nothing. Mac `E74` is
  `<*>-_continuousScroll is deprecated…` and the real content starts at the
  hyphen. `.+?` misses it.
- **No collapse of wildcard runs.** You keep one group for each `<*>`.
- **`content.strip()`** — this is not optional. The `Content` values in
  HealthApp all start with a space (` getTodayTotalDetailSteps = …`), which
  costs 12% of that dataset. logparser does the same strip at
  `regexmatch.py:194`.

Confirmed results:

```
Linux E18 vs the real two-space content:
  {'p1': '0', 'p2': '0', 'p3': '220-135-151-1.hinet-ip.hinet.net', 'p4': 'root'}

Mac E152 (newline in the template) vs content with a space:  matches

(DiskStore.Normal:<*>) <*> <*> vs "(DiskStore.Normal:2) a b c":
  {'p1': '2', 'p2': 'a', 'p3': 'b c'}   <- all three groups kept
```

### Measured match rate

All 1347 templates against the 32000 `Content` values of the structured files:

| dataset | this recipe | logparser rule |
|---|---|---|
| Android, Apache, BGL, HDFS, HPC, Hadoop, HealthApp, Linux, Mac, OpenSSH, OpenStack, Proxifier, Spark, Windows, Zookeeper | 100.0% | 83.2% – 100% |
| Thunderbird (file as shipped) | 58.7% | 58.7% |
| Thunderbird (after the quote repair) | 100.0% | — |
| **total, as shipped** | **97.4%** | **94.3%** |

The logparser rule loses ground mostly on Hadoop (83.2%), Mac (89.6%) and HPC
(92.9%). The cause is the literal-space rule.

With the one-character Thunderbird repair, this recipe matches **every line of
all 16 datasets**.

---

## 7. Faults in logparser to know about

1. **Parameters break into characters.** `regexmatch.py:218-219`:

   ```python
   if parameter_list:
       parameter_list = list(parameter_list[0])
   ```

   `re.findall` gives a list of **strings** when the pattern holds 0 or 1
   capture group, and a list of **tuples** only when it holds 2 or more. So
   template `size <*>` against `size 1234` gives `['1','2','3','4']`. Only
   templates with two or more wildcards give correct parameters. The recipe in
   section 6 avoids this, because it uses `match` and `groupdict`.

2. **`optimized=True` does not run.** `regexmatch.py:193` calls `.iteritems()`,
   which is Python 2.

3. **`n_workers > 1` does not run** in the installed copy — `xrange` at
   `regexmatch.py:89`, and a float `chunk_size` at line 79. The repository's
   copy, at `$LOGPARSER/logparser/logmatch/regexmatch.py`, repairs both lines.
   Keep `n_workers=1`, or copy that file over the installed one.

4. **Templates accumulate.** `RegexMatch.match` reads the CSV each time but
   never empties `template_match_dict` (`regexmatch.py:41`). Do not call
   `match()` twice on one instance.

5. **No validation.** An empty `EventTemplate` cell becomes `NaN` and then
   fails at `regexmatch.py:64`.

---

## 8. Where things are

| what | path |
|---|---|
| corrected templates | `$LOGPARSER/data/loghub_2k_corrected/<Name>/<Name>_2k.log_templates_corrected.csv` |
| corrected ground truth | same directory, `..._structured_corrected.csv` |
| original set | `$LOGPARSER/data/loghub_2k/` |
| matcher | `$SITE_PACKAGES/logmatch/regexmatch.py` |
| header splitter | `$SITE_PACKAGES/utils/logloader.py` |
| `log_format` strings | `$LOGPARSER/logparser/Drain/benchmark.py:28-155` |

Each corrected dataset directory holds 5 files: the log, and the templates and
structured files in both the original and the corrected form.

`$LOGPARSER/data/` holds four things: `loghub_2k`, `loghub_2k_corrected`,
`test_log` and a `README.md`.

### The `log_format` strings

You need these to remove the header before you match.

```
Android      <Date> <Time>  <Pid>  <Tid> <Level> <Component>: <Content>
Apache       \[<Time>\] \[<Level>\] <Content>
BGL          <Label> <Timestamp> <Date> <Node> <Time> <NodeRepeat> <Type> <Component> <Level> <Content>
HDFS         <Date> <Time> <Pid> <Level> <Component>: <Content>
HPC          <LogId> <Node> <Component> <State> <Time> <Flag> <Content>
Hadoop       <Date> <Time> <Level> \[<Process>\] <Component>: <Content>
HealthApp    <Time>\|<Component>\|<Pid>\|<Content>
Linux        <Month> <Date> <Time> <Level> <Component>(\[<PID>\])?: <Content>
Mac          <Month>  <Date> <Time> <User> <Component>\[<PID>\]( \(<Address>\))?: <Content>
OpenSSH      <Date> <Day> <Time> <Component> sshd\[<Pid>\]: <Content>
OpenStack    <Logrecord> <Date> <Time> <Pid> <Level> <Component> \[<ADDR>\] <Content>
Proxifier    \[<Time>\] <Program> - <Content>
Spark        <Date> <Time> <Level> <Component>: <Content>
Thunderbird  <Label> <Timestamp> <Date> <User> <Month> <Day> <Time> <Location> <Component>(\[<PID>\])?: <Content>
Windows      <Date> <Time>, <Level>                  <Component>    <Content>
Zookeeper    <Date> <Time> - <Level>  \[<Node>:<Component>@<Id>\] - <Content>
```

### Licence

`$LOGPARSER/data/loghub_2k/LICENSE` limits the data to research and academic
use, and asks for a citation of the loghub repository. This applies if you ship
the harvested templates. `loghub_2k_corrected/` holds no licence file of its
own.

The corrected templates come from *"Guidelines for Assessing the Accuracy of
Log Message Template Identification Techniques"*, ICSE 2022.

---

## 9. Worked examples

| dataset | raw line | template |
|---|---|---|
| Mac | `Jul  1 09:01:05 calvisitor-… com.apple.CDScheduler[43]: Thermal pressure state: 1 Memory pressure state: 0` | `E323` `Thermal pressure state: <*> Memory pressure state: <*>` |
| HDFS | `081109 203615 148 INFO dfs.DataNode$PacketResponder: PacketResponder 1 for block blk_38865049064139660 terminating` | `E10` `PacketResponder <*> for block <*> terminating` |
| Linux | `Jun 14 15:16:02 combo sshd(pam_unix)[19937]: check pass; user unknown` | `E27` `check pass; user unknown` (no wildcard) |
| Apache | `[Sun Dec 04 04:47:44 2005] [error] mod_jk child workerEnv in error state 6` | `E3` `mod_jk child workerEnv in error state <*>` |
| Android | `03-17 16:13:38.819  1702  8671 D PowerManagerService: acquire lock=233570404, flags=0x1, tag="View Lock", name=com.android.systemui, ws=null, uid=10037, pid=2227` | `E10` `acquire lock=<*>, flags=<*>, tag="<*>", name=<*>, ws=<*>, uid=<*>, pid=<*>` |

One more Mac line shows how much the parser removes:

```
raw       Jul  1 09:00:55 calvisitor-10-105-160-95 kernel[0]: IOThunderboltSwitch<0>(0x0)::listenerCallback - Thunderbolt HPD packet for route = 0x0 port = 11 unplug = 0
template  E252  <*>(<*>)::listenerCallback - Thunderbolt HPD packet for route = <*> port = <*> unplug = <*>
```

`IOThunderboltSwitch<0>` became `<*>`, angle brackets and all.
