package theme

import (
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// rendererAt is a renderer at the named colour depth, on a dark or a light
// background. Lip Gloss takes the depth as a termenv profile, and this
// module does not import termenv (Lip Gloss's dependency, not its own: ADR
// 0052 section 3), so each depth is given as termenv's number for it; the
// name check makes a renumbering fail here rather than test the wrong depth.
func rendererAt(t *testing.T, depth string, dark bool) *lipgloss.Renderer {
	t.Helper()
	r := lipgloss.NewRenderer(io.Discard)
	switch depth {
	case "TrueColor":
		r.SetColorProfile(0)
	case "ANSI256":
		r.SetColorProfile(1)
	case "ANSI":
		r.SetColorProfile(2)
	case "Ascii":
		r.SetColorProfile(3)
	default:
		t.Fatalf("no colour depth %q", depth)
	}
	if got := r.ColorProfile().Name(); got != depth {
		t.Fatalf("the profile set for %s is %s", depth, got)
	}
	r.SetHasDarkBackground(dark)
	return r
}

// escaped reports whether s carries an escape sequence.
func escaped(s string) bool { return strings.Contains(s, "\x1b") }

// Every colour string in the table is one Lip Gloss can paint. A malformed
// one paints nothing and raises no error, so a typo in tokens.json would
// leave a role uncoloured without a word, and an index past 15 in the
// 16-colour place panics inside termenv when it is drawn. Each value is
// checked for its form, then drawn at its own depth, where it must paint
// exactly when it is set.
func TestThemeEveryTokenParses(t *testing.T) {
	hex := regexp.MustCompile(`^#[0-9a-fA-F]{6}$`)
	if len(termTokens) == 0 {
		t.Fatal("tokens_gen.go holds no term-* token")
	}
	for name, c := range termTokens {
		for _, bg := range []struct {
			name string
			dark bool
			at   lipgloss.CompleteColor
		}{{"dark", true, c.Dark}, {"light", false, c.Light}} {
			if !hex.MatchString(bg.at.TrueColor) || !index(bg.at.ANSI256, 255) || !index(bg.at.ANSI, 15) {
				t.Errorf("%s on %s: %+v; want a #rrggbb truecolor, a 256-colour index (0-255) or empty, "+
					"and a 16-colour index (0-15) or empty", name, bg.name, bg.at)
				continue
			}
			for _, depth := range []struct{ name, value string }{
				{"TrueColor", bg.at.TrueColor}, {"ANSI256", bg.at.ANSI256}, {"ANSI", bg.at.ANSI},
			} {
				out := rendererAt(t, depth.name, bg.dark).NewStyle().Foreground(c).Render("x")
				if escaped(out) != (depth.value != "") {
					t.Errorf("%s on %s at %s: value %q drew %q", name, bg.name, depth.name, depth.value, out)
				}
			}
		}
	}
}

// index reports whether s is empty or a colour index from 0 to top, written
// as Lip Gloss reads one.
func index(s string, top int) bool {
	if s == "" {
		return true
	}
	n, err := strconv.Atoi(s)
	return err == nil && n >= 0 && n <= top && strconv.Itoa(n) == s
}

// tokensJSON is the part of docs/design/system/tokens.json the theme is
// made from.
type tokensJSON struct {
	Color struct {
		Tokens []struct {
			Name  string          `json:"name"`
			Value json.RawMessage `json:"value"`
		} `json:"tokens"`
	} `json:"color"`
	Terminal struct {
		Tokens []struct {
			Name  string `json:"name"`
			Value string `json:"value"`
			Usage string `json:"usage"`
		} `json:"tokens"`
	} `json:"terminal"`
}

// Every term-* token in tokens.json reaches the theme with the hex its
// colour role has in both themes, its 256-colour index, and the 16-colour
// index its usage text names, with a light terminal below truecolor taking
// no colour for text and the 16-colour index for everything else (owner
// decision, 2026-10-25). tokens.json is the one source (DESIGN.md section
// 3): a table that drifted from it paints colours the brand book does not
// give. The test reads tokens.json itself rather than trusting the
// generator, and checks the colour each theme role paints, so a role that
// shares another's entry (warn is signal's) still matches its own token.
func TestThemeMatchesTokensJSON(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "docs", "design", "system", "tokens.json"))
	if err != nil {
		t.Fatal(err)
	}
	var doc tokensJSON
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	if len(doc.Terminal.Tokens) == 0 {
		t.Fatal("tokens.json has no term-* token")
	}
	hexOf := map[string][2]string{} // colour role: dark, light
	for _, c := range doc.Color.Tokens {
		var both struct{ Dark, Light string }
		if json.Unmarshal(c.Value, &both) != nil {
			// A colour with no per-theme split (cool-fill) is the same in both.
			var one string
			if err := json.Unmarshal(c.Value, &one); err != nil {
				t.Fatalf("%s: value %s is neither one colour nor dark and light", c.Name, c.Value)
			}
			both.Dark, both.Light = one, one
		}
		hexOf[c.Name] = [2]string{both.Dark, both.Light}
	}
	sixteen := sixteenFrom(t, doc)

	// At truecolor every role paints a colour; below it the raised row is
	// reverse video and carries none.
	th := New(rendererAt(t, "TrueColor", true))
	painted := map[string]lipgloss.TerminalColor{
		"term-ground":        th.Ground.GetBackground(),
		"term-panel":         th.Panel.GetBackground(),
		"term-panel-raised":  th.PanelRaised.GetBackground(),
		"term-line":          th.Line.GetForeground(),
		"term-line-strong":   th.LineStrong.GetForeground(),
		"term-ink":           th.Ink.GetForeground(),
		"term-ink-secondary": th.InkSecondary.GetForeground(),
		"term-ink-muted":     th.InkMuted.GetForeground(),
		"term-signal":        th.Signal.GetForeground(),
		"term-cool":          th.Cool.GetForeground(),
		"term-play":          th.Play.GetForeground(),
		"term-ok":            th.OK.GetForeground(),
		"term-down":          th.Down.GetForeground(),
		"term-warn":          th.Warn.GetForeground(),
		"term-edge":          th.Edge.GetForeground(),
		"term-edge-active":   th.EdgeActive.GetForeground(),
	}
	for _, tok := range doc.Terminal.Tokens {
		role := strings.TrimPrefix(tok.Name, "term-")
		hex, ok := hexOf[role]
		if !ok {
			t.Errorf("%s: tokens.json has no colour %s", tok.Name, role)
			continue
		}
		dark16, light16 := sixteen[tok.Name][0], sixteen[tok.Name][1]
		if role == "ink" || strings.HasPrefix(role, "ink-") {
			light16 = ""
		}
		want := lipgloss.CompleteAdaptiveColor{
			Dark:  lipgloss.CompleteColor{TrueColor: hex[0], ANSI256: tok.Value, ANSI: dark16},
			Light: lipgloss.CompleteColor{TrueColor: hex[1], ANSI256: light16, ANSI: light16},
		}
		if got := termTokens[tok.Name]; got != want {
			t.Errorf("%s: tokens_gen.go has %+v, tokens.json gives %+v", tok.Name, got, want)
		}
		if got := painted[tok.Name]; got != want {
			t.Errorf("%s: the theme paints %+v, tokens.json gives %+v", tok.Name, got, want)
		}
	}
	if len(termTokens) != len(doc.Terminal.Tokens) {
		t.Errorf("tokens_gen.go has %d term-* tokens, tokens.json %d", len(termTokens), len(doc.Terminal.Tokens))
	}

	// A role with no term-* token paints the entry of the role tokens.json
	// gives the same colour, in both themes.
	for role, s := range map[string]struct {
		style lipgloss.Style
		token string
	}{
		"up":        {th.Up, "term-cool"},
		"diff-hunk": {th.DiffHunk, "term-cool"},
		"diff-add":  {th.DiffAdd, "term-ok"},
		"diff-del":  {th.DiffDel, "term-down"},
	} {
		if got := s.style.GetForeground(); got != termTokens[s.token] {
			t.Errorf("%s paints %+v, not %s's entry", role, got, s.token)
		}
		if same := strings.TrimPrefix(s.token, "term-"); hexOf[role] != hexOf[same] {
			t.Errorf("tokens.json gives %s %v and %s %v: they no longer share an entry",
				role, hexOf[role], same, hexOf[same])
		}
	}
}

// sixteenFrom reads each term-* token's 16-colour index on a dark and on a
// light terminal from its usage text, as DESIGN.md section 3.4 describes:
// the index in parentheses, the one marked "on dark terminals" for a dark
// one where a usage gives two; none for default background or reverse
// video; another token's for "Same as <role>". It is read here apart from
// the generator, so the two readings have to agree.
func sixteenFrom(t *testing.T, doc tokensJSON) map[string][2]string {
	t.Helper()
	indexed := regexp.MustCompile(`\((\d+)\)( on dark terminals)?`)
	out := map[string][2]string{}
	same := map[string]string{}
	for _, tok := range doc.Terminal.Tokens {
		_, text, ok := strings.Cut(tok.Usage, "16-colour:")
		if !ok {
			other, ok := strings.CutPrefix(tok.Usage, "Same as ")
			if !ok || len(strings.Fields(other)) == 0 {
				t.Fatalf("%s: usage %q names no 16-colour value", tok.Name, tok.Usage)
			}
			same[tok.Name] = "term-" + strings.TrimRight(strings.Fields(other)[0], ";,.")
			continue
		}
		var dark, light string
		for _, m := range indexed.FindAllStringSubmatch(text, -1) {
			if m[2] != "" {
				dark = m[1]
			} else {
				light = m[1]
			}
		}
		if dark == "" {
			dark = light
		}
		out[tok.Name] = [2]string{dark, light}
	}
	for name, other := range same {
		v, ok := out[other]
		if !ok {
			t.Fatalf("%s is the same as %s, which gives no 16-colour value", name, other)
		}
		out[name] = v
	}
	return out
}

// With no colour profile (NO_COLOR is set, or the output is not a terminal)
// every style draws its text alone: an escape sequence there reaches a pipe
// or a log as stray characters. Every style is drawn, on both backgrounds,
// and the same theme at truecolor paints every one, so this cannot pass on
// styles that set nothing.
func TestThemeNoColourProfilePaintsNothing(t *testing.T) {
	for _, dark := range []bool{true, false} {
		plain := styles(t, New(rendererAt(t, "Ascii", dark)))
		coloured := styles(t, New(rendererAt(t, "TrueColor", dark)))
		for name, s := range plain {
			if out := s.Render("x"); out != "x" {
				t.Errorf("dark %v: %s drew %q with no colour profile", dark, name, out)
			}
			if out := coloured[name].Render("x"); !escaped(out) {
				t.Errorf("dark %v: %s drew %q at truecolor", dark, name, out)
			}
		}
	}
}

// styles is every style in th by its field name. It walks the struct, so a
// style added to Theme later is drawn by these tests without editing them.
func styles(t *testing.T, th Theme) map[string]lipgloss.Style {
	t.Helper()
	v := reflect.ValueOf(th)
	out := map[string]lipgloss.Style{}
	for i := 0; i < v.NumField(); i++ {
		s, ok := v.Field(i).Interface().(lipgloss.Style)
		if !ok {
			t.Fatalf("Theme.%s is not a lipgloss.Style", v.Type().Field(i).Name)
		}
		out[v.Type().Field(i).Name] = s
	}
	if len(out) == 0 {
		t.Fatal("Theme has no style")
	}
	return out
}

// A raised row (hover, selection) is the panel-raised fill where the
// terminal has a colour for it and reverse video where it has none, as
// term-panel-raised's usage says. Drawn as a fill that is not there, a
// selected row on a 16-colour terminal would look like every other row.
func TestThemeRaisedRowIsReverseVideoWithoutAFill(t *testing.T) {
	for _, c := range []struct {
		depth         string
		dark, reverse bool
	}{
		{"TrueColor", true, false},
		{"ANSI256", true, false},
		{"ANSI", true, true},
		{"TrueColor", false, false},
		{"ANSI256", false, true},
		{"ANSI", false, true},
	} {
		out := New(rendererAt(t, c.depth, c.dark)).PanelRaised.Render("x")
		if !escaped(out) || strings.Contains(out, "\x1b[7m") != c.reverse {
			t.Errorf("%s, dark %v: a raised row drew %q; want reverse video %v", c.depth, c.dark, out, c.reverse)
		}
	}
}
