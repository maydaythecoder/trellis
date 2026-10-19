# Vulnerability exceptions for tui/

None. A vulnerability govulncheck reports in tui/'s module graph is never
ignored (ADR 0052 section 5, as ADR 0048 section 7 holds for Tauri's tree).
It fails G5 and CI's `supply-chain` job until a fixed version is in
`go.sum`. If no fixed release exists, the Go TUI does not ship (ADR 0052,
"What would reverse it"). This file lists no exception and is not a place
to add one.

`golang.org/x/sys` is required at v0.44.0, above what the Charm set asks
for, because v0.38.0 carries GO-2026-5024 (`golang.org/x/sys/windows`,
fixed in v0.44.0). govulncheck reported it as not reached from tui/'s code
on 2026-10-12. It was moved anyway rather than left as an unreached finding.
