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
echo "OK: no stub markers in added production lines"
