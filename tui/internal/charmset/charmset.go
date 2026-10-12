// Package charmset pins the Charm libraries as one set (ADR 0052 section
// 3): Bubble Tea, Bubbles and Lip Gloss move together in one commit, as
// tree-sitter and tree-sitter-python do. Bubbles v1.0.0 was released
// against Bubble Tea v1.3.10 and Lip Gloss v1.1.0, the versions go.mod
// requires.
//
// The screen imports Bubble Tea (internal/model) and Lip Gloss
// (internal/view), not Bubbles; these blank imports keep `go mod tidy` from
// dropping any of the set from go.mod and go.sum, so the supply-chain
// checks (go mod verify, govulncheck, go-licenses) read the whole set the
// TUI is pinned to.
package charmset

import (
	_ "github.com/charmbracelet/bubbles/textinput"
	_ "github.com/charmbracelet/bubbletea"
	_ "github.com/charmbracelet/lipgloss"
)
