# Pyright backstop against real Pyright, on poetry 2.5.0 (2026-09-29)

#168 built the backstop (ADR 0031) and tested it against a scripted language
server. This is its first run against Pyright itself (#177).

Nothing here is a recall figure. "Bound by pyright" counts sites where
Pyright's answer passed the binder's filters; whether those edges are the
runtime callee is measured by the human audit (#3), not by agreement with
Pyright (ADR 0031, "Agreement with Pyright is not correctness").

## Setup

| | |
|---|---|
| trellis commit | `288e7aa2736da6aeec450461488f214d1dd4fb1c` (branch `t177-real-pyright`; the backstop and CLI are unchanged by this ticket) |
| build | `cargo build --release -p trellis-cli` |
| Pyright | 1.1.414, `npm install --prefix C:/w/pyright-tool pyright@1.1.414` (scratch prefix, not global; its `node_modules/.bin` put on `PATH` for the run only) |
| Node | v25.6.0 |
| host | Windows 11 Home 10.0.26200 |
| corpus | `.bench/poetry-2.5.0` (see `README.md` here), copied without its `.trellis/` and `.mypy_cache/` to a scratch directory, because `trellis index` writes `.trellis/index.bin` into the target |
| config | the copy's `.trellis/config.toml`: `[index]` / `backstop = "pyright"` |
| Python environment | **none.** No virtualenv and none of poetry's dependencies installed, so Pyright resolved the standard library and first-party code only. A receiver typed through `cleo`, `poetry-core` or another dependency could not be followed. `left alone` below is larger than it would be with the dependencies installed; how much larger was not measured. |

## Before the full run: the definition range

The likeliest silent failure named in #177 was Pyright's definition range
starting at `def`/`class` rather than at the name, which the binder requires
(`symbol_named_at`). One site was asked by hand with a 40-line Python LSP
client, before `trellis index` was run: `merge_dicts(...)` at
`src/poetry/config/config.py:223`, zero-based position 222:8. Pyright answered

```
{"uri": "file:///c%3A/w/t177-corpus/src/poetry/utils/helpers.py",
 "range": {"start": {"line": 109, "character": 4}, "end": {"line": 109, "character": 15}}}
```

Line 110 (one-based), column 4 is the `m` of `def merge_dicts(`: the range is
the name. It also shows the URI form Pyright uses on Windows (`c%3A`, lower
case), which `uri_to_path` plus `canonicalize` handles. The rest of #168's
unverified list, as observed:

- `initialize` parameters, `didOpen`, the `workspace/configuration` reply
  (`null` per item): Pyright accepted all three and answered every query.
- The Windows `pyright-langserver.cmd` fallback: npm installs no `.exe`
  (the bin directory holds an extensionless shell script, `.cmd` and
  `.ps1`), so the bare name is not found and the server that answered was
  started through the `.cmd` fallback.
- **`serverInfo`: Pyright 1.1.414 does not send it.** Both the hand client and
  trellis saw no `serverInfo` in the `initialize` result, so `trellis index`
  prints "the server did not report its version". ADR 0031's first slice says
  the version is printed in `trellis index` output; against real Pyright it is
  not. The version on this page comes from npm, not from the server.

## The run

Command, from PowerShell 7, via
[`pyright_backstop_measure.ps1`](pyright_backstop_measure.ps1):

```
pyright_backstop_measure.ps1 -Exe target/release/trellis.exe -Root <copy> -Out run1.txt -PyrightBin C:/w/pyright-tool/node_modules/.bin
```

which runs `trellis --root <copy> index`. The committed script is the one
that was run, with the Pyright bin directory made a parameter (the run had it
written in). Output: the first lines and the backstop section verbatim, the
resolver's reason and shape tables elided as `...` (the counts match
`BENCHMARK_AND_EVAL.md` §2). The whitespace inside two of the backstop lines
is how #168's CLI prints them:

```
indexed 451 files in 2.4623855s
  definitions:     3735
  call sites:      21794
  index.bin:       492.6 KiB
  first-party:     13510 (the recall denominator)
  external:        8284 (excluded: stdlib, site-packages)

5908 of 13510 first-party call sites resolved (43.7%); 865 ambiguous, 6737 unresolved; 8284 external (excluded)
...
backstop: pyright ran (the server did not report its version)
  queried            7580  Unresolved or Candidates sites only
  bound by pyright   1859  counted apart: not in `resolved`, not in the graph
  left alone         5721  no answer, or one this backstop would not bind
  not queried          22  keep the resolver's verdict
  A pyright-bound edge is the server's reading of the annotations, not a          verified callee;
  those sites stay unpartitionable.
```

| | count |
|---|---:|
| doubt sites (`Unresolved` 6,737 + `Candidates` 865) | 7,602 |
| queried | 7,580 |
| not queried | 22 |
| bound by pyright | 1,859 |
| left alone | 5,721 |

The 22 not queried are the 22 `DynamicAttribute` sites (`d["k"]()`,
`f()()`): the callee has no identifier to ask about. `left alone` does not
separate "Pyright returned nothing" from "Pyright answered and the binder
refused it" (outside the repository, not a definition's name, more than one
symbol, outside a `Candidates` set); `BackstopReport` has one count for both.

Of the 1,859 bound: 1,002 were `Unresolved` sites and 857 were `Candidates`
sites (from the edge listing below).

The same counts (7,580 / 1,859 / 5,721 / 22) came back on four further runs
through the example below. The second and third listings are byte-identical;
the fourth differs from them only where a carriage return inside a call's
text is now printed as a space; the first was not kept.

## Wall time and peak memory

Method: `pyright_backstop_measure.ps1`. A stopwatch from process start to
exit; every 500 ms, `PeakWorkingSet64` of `trellis.exe` and of each
`node.exe` whose command line contains `pyright`. `PeakWorkingSet64` is the
OS's running maximum, so sampling can miss only growth in the last interval
before a process exits. One run (n = 1), release build, on a machine running
other builds at the same time.

| | |
|---|---:|
| wall time, whole `trellis index` (resolver index 2.46 s included) | 59.3 s |
| peak working set, `node.exe` (Pyright) | 677.5 MiB |
| peak working set, `trellis.exe` | 55.3 MiB |

The two peaks are separate processes and are not added. Poetry is 451 files;
this says nothing about ADR 0031's "multi-GB on large repositories", which
stays pending for a large repository.

## What the edges are

`trellis index` prints counts only. To read the edges,
`crates/trellis-index/examples/backstop_edges.rs` runs the same backstop and
prints each bound site beside its callee:

```
cargo run --release -p trellis-index --example backstop_edges -- <copy>
```

**Name check: 0 of 1,859** bound edges name a symbol whose name differs from
the identifier the call calls (`check_soundness`'s condition, applied to the
backstop's edges). Necessary, not sufficient, as there.

**Hand-checked sites** (read in the source, by the implementer):

1. `src/poetry/config/config.py:443`, `_default_config.merge(config_file.read())`,
   resolver `Unresolved`, bound to `Config.merge` (config.py:220).
   `_default_config` is declared `Config | None` at line 148 and assigned
   `cls()` inside `Config.create`, guarded by `is None`. Correct for `Config`;
   see the next section for what it does not cover.
2. `src/poetry/console/commands/installer_command.py:21`, `super().__init__()`
   in `class InstallerCommand(GroupCommand, EnvCommand)`, resolver
   `Unresolved`, bound to `EnvCommand.__init__` (env_command.py:13).
   `GroupCommand` defines no `__init__`, so the MRO reaches `EnvCommand`
   next. Correct.
3. `src/poetry/config/config_source.py:63`, `config_source.get_property(...)`
   with `config_source: ConfigSource`, resolver `Candidates(3)`, bound to
   `ConfigSource.get_property` (config_source.py:44), which is an
   `@abstractmethod`. The runtime callee is one of the two overrides, never
   the method bound. See below.

## Finding: Pyright answers with the declared type's member

`textDocument/definition` names the member of the receiver's *declared*
type. ADR 0012's first invariant says a declared type is an upper bound, not
an identity: when a first-party subclass overrides the method, the resolver
keeps the base and every override as `Candidates`. The binder accepts any
single answer inside a `Candidates` set, so for those sites Pyright's answer
is usually the base the resolver already had, and the overrides are dropped.

Measured over the 1,859 edges with
[`backstop_overrides.py`](backstop_overrides.py) (a text proxy over class
lines, not the resolver's hierarchy; classes matched by bare name):

| bound edges | total | callee is a method | callee overridden by a first-party subclass |
|---|---:|---:|---:|
| from `Candidates` sites | 857 | 840 | 813 |
| from `Unresolved` sites | 1,002 | 953 | 150 |

A stricter count by the same script, callees that are `@abstractmethod` or
have a stub body (`...`, `pass`, `raise NotImplementedError`), and so cannot
be the runtime callee at all: 8 from `Candidates` sites and 30 from
`Unresolved` sites.

What this does and does not show: an edge to an overridden method is not
necessarily wrong for that site (the receiver may really be the base class),
but it is not the whole answer, and under ADR 0012 the resolver would not have
bound it. The same property holds for the `Unresolved` sites: had a subclass
of `Config` overridden `merge`, site 1 above would still bind `Config.merge`.
So the 1,859 is not a count of sites narrowed to their runtime callee. None of
these edges enter the graph or `resolved` (ADR 0031), so nothing downstream
consumes them yet; the count is what a reader sees.

The binder was not changed here: #177's fix clause is for a bound count of
zero, and whether an overridden answer should be refused, on both kinds of
site, changes what "bound by pyright" means, which is ADR 0031's decision.

## `check_soundness`

```
cargo run --release -p trellis-index --example check_soundness -- <copy>
checked 5908 Resolved edges (0 had no comparable identifier)
No edge names a symbol the source did not call.
```

Zero violations, as expected: the backstop's edges are not in `Index::edges`,
so the resolver's output is unchanged by it.

## Re-run with overridden and abstract answers refused (#186, 2026-09-29)

#186 changed the binder: an answer whose one symbol is a method a
first-party subclass overrides, or a function or method that is
`@abstractmethod` or has a stub body, is now refused and counted as
**refused: overridden or abstract** (ADR 0031, amendment of 2026-09-29).

Method: as above, changing only the trellis commit.

| | |
|---|---|
| trellis commit | `9a644bbf4abf774d963b1f27d135a62f40acd28d` (branch `t186-refuse-overridden`) |
| build | `cargo build --release -p trellis-cli`; the two examples with `cargo build --release -p trellis-index --example backstop_edges --example check_soundness` |
| Pyright, Node, host | as above: 1.1.414 from the same scratch prefix, v25.6.0, Windows 11 Home 10.0.26200 |
| corpus | a fresh copy of `.bench/poetry-2.5.0` without `.trellis/` and `.mypy_cache/`, with the same `.trellis/config.toml`; its `src/` compared equal (`diff -r`) to #177's copy |
| Python environment | none, as above |

`pyright_backstop_measure.ps1` was run twice. The resolver's line is the
same as #177's (`5908 of 13510 ... 865 ambiguous, 6737 unresolved`), and the
backstop section was identical in both runs:

```
backstop: pyright ran (pyright 1.1.414, from its package.json)
  queried            7580  Unresolved or Candidates sites only
  bound by pyright    827  counted apart: not in `resolved`, not in the graph
  no answer          2949  the server returned no location; resolver's verdict kept
  refused            3804  an answer this backstop would not bind; resolver's verdict kept
    outside the repository         2192
    not at a definition's name      568
    more than one symbol             12
    overridden or abstract         1032
  not queried          22  keep the resolver's verdict
```

| | #177 | #186 |
|---|---:|---:|
| queried | 7,580 | 7,580 |
| bound by pyright | 1,859 | 827 |
| refused: overridden or abstract | (not a reason then) | 1,032 |
| no answer + every other refusal | 5,721 | 5,721 (2,949 + 2,192 + 568 + 12) |
| not queried | 22 | 22 |

The new refusal took its 1,032 entirely from what #177 bound
(1,859 − 827); no other count moved.

Bound edges by the resolver's verdict, from `backstop_edges` on the same
copy: **800 from `Unresolved` sites and 27 from `Candidates` sites** (#177:
1,002 and 857). By subtraction, the refusals were 202 `Unresolved` and 830
`Candidates` sites. That subtraction assumes Pyright answered as it did for
#177; the unchanged counts above are consistent with that, and it was not
checked site by site.

The #177 text proxy ([`backstop_overrides.py`](backstop_overrides.py)) over
the 827 remaining edges finds 0 whose callee a subclass overrides and 0
abstract or stub callees. Its #177 counts (813 + 150 overridden, 8 + 30
abstract or stub) differ from the 1,032 refused here, and there are known
reasons they would: the proxy matches classes by bare name and reads bases
from the `class` line, where the binder uses the resolver's hierarchy; the
proxy checks the first statement of a body, where the binder requires every
statement after the docstring to be `...`, `pass` or `raise
NotImplementedError`; and the binder applies the abstract and stub test to
module-level functions too (an `@overload` signature is a `...` body). How
much each contributes was not measured.

Name check over the 827: 0 name a symbol whose name differs from the call's
identifier.

`check_soundness` on the same copy:

```
checked 5908 Resolved edges (0 had no comparable identifier)
No edge names a symbol the source did not call.
```

Zero violations. The refactor that lets the backstop ask the resolver's
override rule (`prepare` split out of `build_from_facts`) left the
resolver's counts on this corpus as #177 printed them (the line above and
the resolver's line in `trellis index`), and every `trellis-index` test and
snapshot passes; that is evidence the resolver's edges did not change, not a
byte comparison of them. Precision on the audited sample is a property of the resolver's
edges, which did not change; the audit was not re-run.

Wall time and peak working set, one run each, same method as above, on a
machine running other builds: 23.3 s and 26.6 s for the whole `trellis
index`; `node.exe` (Pyright) 711.2 MiB and 717.2 MiB; `trellis.exe`
56.7 MiB and 47.0 MiB. The backstop now re-reads and re-parses the indexed
files before starting the server (the index stores no class hierarchy); that
cost is inside these figures and was not measured apart. The second run
found `.trellis/index.bin` written by the first.
