// Package rpc starts one `trellis --root <repo> rpc` child and speaks to it
// in JSON lines (ADR 0025 section 5).
//
// It is the only package under tui/ that starts a process
// (scripts/checks/go-guards.sh holds that), and it follows
// desktop/src/child.rs:
//
//   - one argument list, `--root <repo> rpc`, run in <repo>;
//   - one command per line on stdin; each stdout line delivered in order,
//     as JSON when it parses and as text when it does not, so nothing the
//     child printed is dropped;
//   - stderr is drained so the child never blocks on it, and kept only as
//     human text in a capped buffer for a pane (ADR 0052 section 4); it is
//     never parsed (ADR 0048 section 2);
//   - shutdown writes {"type":"quit"} and waits. Nothing here kills the
//     child: a run in flight then finishes, rolls back and writes its
//     trail, which a kill would skip (ADR 0048 sections 3 and 9);
//   - the environment is inherited unchanged and no key variable is read
//     (ADR 0010, ADR 0024 section 3).
package rpc

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
)

// StderrCap is how many of the child's stderr lines are kept. Older lines
// are dropped first and counted, so a pane can say that some were.
const StderrCap = 500

// Line is one stdout line from the child.
type Line struct {
	// Text is the line as read, without its line ending.
	Text string
	// JSON is the line parsed as a JSON object, or nil when it is not one.
	JSON map[string]any
}

// Parsed reports whether the line was a JSON object.
func (l Line) Parsed() bool { return l.JSON != nil }

// Type is the object's "type" field, or "" when there is none.
func (l Line) Type() string {
	if l.JSON == nil {
		return ""
	}
	s, _ := l.JSON["type"].(string)
	return s
}

// ParseLine turns one stdout line into a Line. A line that is not a JSON
// object keeps its text and has no JSON.
func ParseLine(text string) Line {
	text = strings.TrimRight(text, "\r\n")
	var obj map[string]any
	if err := json.Unmarshal([]byte(text), &obj); err != nil || obj == nil {
		return Line{Text: text}
	}
	return Line{Text: text, JSON: obj}
}

// Client is a running `trellis rpc` child.
type Client struct {
	cmd *exec.Cmd

	writeMu sync.Mutex
	stdin   io.WriteCloser
	quit    bool

	lines chan Line

	errMu      sync.Mutex
	stderr     []string
	stderrLost int

	exited  chan struct{}
	waitErr error
}

// ErrQuit is returned by Send after Quit: the child's stdin is closed.
var ErrQuit = errors.New("rpc: quit already sent; the child's stdin is closed")

// Start runs `<trellis> --root <root> rpc` in <root>.
func Start(trellis, root string) (*Client, error) {
	if trellis == "" {
		return nil, errors.New("rpc: no trellis binary named")
	}
	if root == "" {
		return nil, errors.New("rpc: no root named")
	}
	cmd := exec.Command(trellis, "--root", root, "rpc")
	cmd.Dir = root
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("rpc: stdin pipe: %w", err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("rpc: stdout pipe: %w", err)
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, fmt.Errorf("rpc: stderr pipe: %w", err)
	}
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("rpc: start %s: %w", trellis, err)
	}
	c := &Client{
		cmd:    cmd,
		stdin:  stdin,
		lines:  make(chan Line, 256),
		exited: make(chan struct{}),
	}
	var readers sync.WaitGroup
	readers.Add(2)
	go func() {
		defer readers.Done()
		c.readStdout(stdout)
	}()
	go func() {
		defer readers.Done()
		c.readStderr(stderr)
	}()
	// Wait only after both pipes reach EOF: exec.Cmd.Wait closes them, and a
	// line still unread would be lost.
	go func() {
		readers.Wait()
		c.waitErr = cmd.Wait()
		close(c.exited)
	}()
	return c, nil
}

// readStdout delivers every line in order and closes Lines at EOF. A line
// longer than bufio's default is read whole: a get_codebase answer can be
// large.
func (c *Client) readStdout(r io.Reader) {
	defer close(c.lines)
	br := bufio.NewReader(r)
	for {
		text, err := br.ReadString('\n')
		if text != "" {
			c.lines <- ParseLine(text)
		}
		if err != nil {
			return
		}
	}
}

// readStderr keeps the newest StderrCap lines as text, so the child never
// blocks writing to a full pipe, and nothing reads them as data.
func (c *Client) readStderr(r io.Reader) {
	br := bufio.NewReader(r)
	for {
		text, err := br.ReadString('\n')
		if text != "" {
			c.errMu.Lock()
			c.stderr = append(c.stderr, strings.TrimRight(text, "\r\n"))
			if over := len(c.stderr) - StderrCap; over > 0 {
				c.stderr = append([]string(nil), c.stderr[over:]...)
				c.stderrLost += over
			}
			c.errMu.Unlock()
		}
		if err != nil {
			return
		}
	}
}

// Lines delivers each stdout line in order, and is closed once the child
// has closed its stdout. A caller that stops reading stops the child at its
// next write, so read until it closes.
func (c *Client) Lines() <-chan Line { return c.lines }

// Stderr is a copy of the newest stderr lines kept, and how many older ones
// were dropped. Human text only.
func (c *Client) Stderr() ([]string, int) {
	c.errMu.Lock()
	defer c.errMu.Unlock()
	return append([]string(nil), c.stderr...), c.stderrLost
}

// Send writes one command as one JSON line.
func (c *Client) Send(cmd map[string]any) error {
	b, err := json.Marshal(cmd)
	if err != nil {
		return fmt.Errorf("rpc: encode command: %w", err)
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if c.quit {
		return ErrQuit
	}
	if _, err := c.stdin.Write(append(b, '\n')); err != nil {
		return fmt.Errorf("rpc: write command: %w", err)
	}
	return nil
}

// Quit writes {"type":"quit"} and closes the child's stdin, once. The child
// then aborts any run, waits for its rollback and exits by itself; keep
// reading Lines until it closes, then Wait.
func (c *Client) Quit() error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if c.quit {
		return nil
	}
	c.quit = true
	_, werr := c.stdin.Write([]byte(`{"type":"quit"}` + "\n"))
	cerr := c.stdin.Close()
	if werr != nil {
		// A child that already exited closed its end; Wait says how it ended.
		return fmt.Errorf("rpc: write quit: %w", werr)
	}
	if cerr != nil {
		return fmt.Errorf("rpc: close stdin: %w", cerr)
	}
	return nil
}

// Exited is closed once the child has exited and its pipes are drained.
func (c *Client) Exited() <-chan struct{} { return c.exited }

// Wait blocks until the child has exited by itself and returns how it
// ended. It never kills.
func (c *Client) Wait() error {
	<-c.exited
	return c.waitErr
}

// Shutdown sends quit, discards any stdout lines still to come, and waits
// for the child to exit by itself.
func (c *Client) Shutdown() error {
	qerr := c.Quit()
	for range c.lines {
	}
	werr := c.Wait()
	if werr != nil {
		return fmt.Errorf("rpc: trellis rpc exited: %w", werr)
	}
	return qerr
}
