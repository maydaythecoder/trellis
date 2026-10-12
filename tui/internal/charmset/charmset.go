// Package charmset pins the Charm libraries as one set (ADR 0052 section
// 3): Bubble Tea, Bubbles and Lip Gloss move together in one commit, as
// tree-sitter and tree-sitter-python do. Bubbles v1.0.0 was released
// against Bubble Tea v1.3.10 and Lip Gloss v1.1.0, the versions go.mod
// requires.
//
// Until the screen imports all three (#510), these blank imports keep
// `go mod tidy` from dropping the set from go.mod and go.sum, so the
// supply-chain checks (go mod verify, govulncheck, go-licenses) read the
// tree the TUI will be built from.
package charmset

import (
	_ "github.com/charmbracelet/bubbles/textinput"
	_ "github.com/charmbracelet/bubbletea"
	_ "github.com/charmbracelet/lipgloss"
)
