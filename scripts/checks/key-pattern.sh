#!/usr/bin/env bash
# Prints the extended regex for key-shaped literals: the shapes providers and
# platforms actually issue. G5 (scripts/gates/security.sh) and the pre-commit
# hook (scripts/hooks/pre-commit) both read it from here, so the gate and the
# hook cannot drift apart (#206).
printf '%s\n' '(sk-(ant-|proj-|live-)?[A-Za-z0-9_-]{20,})|(AKIA[0-9A-Z]{16})|(gh[pousr]_[A-Za-z0-9]{36})|(github_pat_[A-Za-z0-9_]{22,})|(xox[baprs]-[A-Za-z0-9-]{10,})|(-----BEGIN [A-Z ]*PRIVATE KEY-----)|(AIza[0-9A-Za-z_-]{35})'
