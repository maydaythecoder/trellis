#!/usr/bin/env bash
# G4: no stub landed in production code. Only lines this change added, and
# only above a file's test module, so a test's `panic!("not a ...")` or a
# marker that predates the change does not fail it.
source "$(dirname "$0")/lib.sh"
hits=$(added_production_lines | grep -E 'TODO|FIXME|XXX|unimplemented!|todo!\(|panic!\("not |NotImplemented|mock_data|MOCK_' || true)
if [ -n "$hits" ]; then
  echo "FAIL: stub markers in production code this change added:"
  printf '%s\n' "$hits"
  exit 1
fi
# Go branch (ADR 0052, #504): added non-test `*.go` lines under tui/, with
# the markers above that apply to Go and Go's form of a refusing stub,
# `panic("not ...")`. Written with `-e` on purpose: trellis-tickets reads the
# single extended-grep pattern above as G4's Rust markers, and refuses a
# file holding a second one in that form (crates/trellis-tickets/src/scan.rs,
# markers_from).
if go_module_present; then
  hits=$(added_production_go_lines | grep -E -e 'TODO|FIXME|XXX|panic\("not |NotImplemented|mock_data|MOCK_' || true)
  if [ -n "$hits" ]; then
    echo "FAIL: stub markers in production Go this change added:"
    printf '%s\n' "$hits"
    exit 1
  fi
  gomsg="; added production Go lines under tui/ too"
else
  gomsg="; Go branch skipped: tui/ is absent"
fi
echo "OK: no stub markers in added production lines$gomsg"
