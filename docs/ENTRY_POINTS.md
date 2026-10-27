# Entry points

Where each program starts, and where the flow goes next, in reading order.
Each step names a file and the function or type to open there, written as a
pair, the path then the symbol, both in code: search the file for the
symbol. Symbols, not line numbers, because line numbers move.
`scripts/checks/entry-points.py` reads every such pair on this page and
fails when the file is gone or the symbol no longer occurs in it, and every
path named alone must exist too, so a rename or a move that leaves this page
behind fails CI rather than going stale.

There are three programs. `trellis`, the Rust binary in `crates/trellis-cli`,
does the work: the index, the agent loop, the checks, rollback and the
trail. The desktop (`desktop/`, Tauri) and the TUI (`tui/`, Go) are clients
of `trellis rpc`: each starts one `trellis --root <repo> rpc` child, writes
it one JSON command per line on stdin, and draws the JSON lines it prints on
stdout. Neither client computes a verdict or a figure the engine did not
report (`docs/design/DESIGN.md` §2), and neither ever kills its child: it
sends `quit` and waits, so a run in flight rolls back and writes its trail.

## trellis

1. `crates/trellis-cli/src/main.rs` `fn main` parses the command line and
   calls `crates/trellis-cli/src/startup.rs` `fn init_tracing`, which builds
   the tracing subscriber. With `--json` or `rpc`, stdout is the JSON
   channel (ADR 0025 §4): the stdout event layer,
   `crates/trellis-cli/src/startup.rs` `let stdout_json`, writes each
   `trellis` event at info as one JSON line, under a fixed filter so
   `RUST_LOG` cannot change what a client receives. Then `main` finds the
   root (`startup.rs` `fn resolve_root`) and `crates/trellis-cli/src/main.rs`
   `fn dispatch` runs the subcommand; `rpc` goes to
   `crates/trellis-cli/src/main.rs` `Command::Rpc => rpc::serve(`.
2. `crates/trellis-cli/src/rpc.rs` `fn serve_with` reads commands until
   `quit` or the end of input, and
   `crates/trellis-cli/src/rpc.rs` `fn dispatch_command` matches each
   command's `type`: `prompt` starts a run, `steer` and `follow_up` queue
   input for the run in flight, `abort` stops it, and the read-only queries
   (`get_*`, `list_*`, `vault_*`) are answered without starting a run. A
   prompt and the follow-ups sent during it run one after another on a
   thread of their own, `crates/trellis-cli/src/rpc.rs` `struct Chain`, so
   the reader keeps reading while a run is in flight.
3. `crates/trellis-cli/src/run_cmd.rs` `pub(crate) fn cmd_run_with` is one
   run, the path `trellis run` also takes; it frames the run's output and
   calls `crates/trellis-cli/src/run_cmd.rs` `fn run_body`. `run_body`
   refuses what it can before it touches the tree (the control mode, trust,
   whether the model is reachable), then takes the snapshot,
   `crates/trellis-vcs/src/lib.rs` `pub fn snapshot` (a clean worktree is
   required, CLAUDE.md rule 7), builds the one writer,
   `crates/trellis-loop/src/orchestrator.rs` `pub struct Orchestrator`, and
   races the agent loop against an interrupt (Ctrl-C, or rpc's `abort`, which
   takes the same path). Any outcome that is not convergence is rolled back,
   and `crates/trellis-cli/src/run_cmd.rs` `fn record_trail` writes the
   trail.
4. `crates/trellis-loop/src/agent.rs` `pub async fn run_seeded` is the
   agent loop. Each turn asks the model, carries out the tool call it chose
   through the orchestrator's `crates/trellis-loop/src/orchestrator.rs`
   `pub struct Handle`, and the loop stops on convergence, the budget or an
   interrupt, returning the outcome and the transcript to `run_body`.

A check's pass or fail is only ever a
`crates/trellis-checks/src/verdict.rs` `pub struct Verdict`, which nothing
outside `trellis-checks` can construct (CLAUDE.md rule 1). `ARCHITECTURE.md`
has the crate map.

## The desktop

1. `desktop/src/main.rs` `fn main` reads `--root`, `--trellis` and `--web`
   and builds the window with the page's four commands. Once the page has
   loaded, it calls `desktop/src/commands.rs` `pub fn startup`, which finds
   the repository and starts the child; `desktop/src/child.rs` is the one
   file that spawns trellis. With `--web` there is no window:
   `desktop/src/web.rs` `pub fn serve` serves the same page and commands on
   127.0.0.1 (ADR 0050 §5).
2. `desktop/src/commands.rs` `pub struct Core` is the window's state.
   Each line the child prints on stdout goes through
   `desktop/src/commands.rs` `fn stdout_line`: it is emitted to the page as
   `rpc-frame`, typed by `desktop/src/frames.rs` `pub fn parse_stdout`, and
   handed to the reducer by `desktop/src/commands.rs` `fn feed`. Stderr
   lines are only ever shown as text (desktop/CLAUDE.md). What the page sends
   reaches the child through `desktop/src/commands.rs` `pub fn rpc_send`,
   which forwards only the types in `desktop/src/commands.rs`
   `pub const FORWARDED` and refuses any other, `quit` among them: only the
   core sends that.
3. `desktop/src/state.rs` `pub fn update` is the reducer: it folds one frame
   into `desktop/src/state.rs` `pub struct DesktopState` and returns the
   panes it changed, and `desktop/src/state.rs` `pub fn view` turns those
   panes into the `view` event the page receives.
4. `desktop/ui/main.js` `listen("view"` is the page's entry module: it
   hands each `view` payload to the draw functions it imports (render.js,
   composer.js, model.js, status.js and the rest) and computes nothing. The
   other modules `desktop/ui/index.html` loads start themselves and listen
   for their own events. Every string reaches the DOM through `textContent`.

### Adding a page module

1. Write `desktop/ui/<name>.js` as an ES module. If it starts itself, add
   `<script type="module" src="<name>.js"></script>` beside the other module
   tags in `desktop/ui/index.html`, with no body and nothing inline
   (`desktop/src/main.rs`
   `fn page_loads_only_module_scripts_and_nothing_inline`). A module that
   only main.js imports needs no tag.
2. Add it to `desktop/src/web.rs` `pub const PAGE`, which serves the page
   for `--web`; `desktop/src/web.rs`
   `fn web_serves_the_files_under_ui_and_nothing_else` fails until it is
   there.
3. If it listens for an event the page did not take before, add the name to
   `desktop/ui/web.js` `const EVENTS`; `desktop/src/web.rs`
   `fn web_js_is_the_one_module_that_fetches_and_only_from_its_own_path`
   holds that list against every name the core emits.
4. There is no list to edit in `desktop/src/lockdown.rs`: its page tests
   read every `.js` file under `desktop/ui/` and follow the page's script
   tags and their imports, `desktop/src/lockdown.rs` `fn loaded_modules`. A
   view key that only a module the page never loads reads is reported unread.
5. Run `scripts/checks/desktop-guards.sh`: its `ui-*` rules hold the new file
   as they hold every other.

## The TUI

1. `tui/cmd/trellis-tui/main.go` `func main` calls
   `tui/cmd/trellis-tui/main.go` `func run`, which parses `--root`,
   `--trellis`, `--model`, `--theme` and `--register`, takes the trellis beside its own
   executable when none is named (never one on PATH), reads the terminal's
   colours once for the theme (`tui/internal/theme/`), starts the child,
   builds the model and runs the Bubble Tea program until the person quits
   or the child exits. However it ended, `tui/cmd/trellis-tui/main.go`
   `func finish` then sends the child `quit` and waits for it, unless it has
   already exited.
2. `tui/internal/rpc/rpc.go` `func Start` runs `trellis --root <root> rpc`,
   delivers its stdout lines in order on a channel and keeps its stderr
   apart, capped. `tui/internal/rpc/rpc.go` `func (c *Client) Send` writes
   one command line; `tui/internal/rpc/rpc.go` `func (c *Client) Quit` sends
   `quit` and closes stdin, after which the child ends its run and exits by
   itself.
3. `tui/internal/frames/frames.go` `func Parse` types one stdout line as a
   `Frame`. It never fails: a line that is not a JSON object is `Unparsed`
   and shown as text.
4. `tui/internal/model/app.go` `func (a *App) Update` is the Bubble Tea
   program and holds all the IO: it hands each parsed line, key and clock
   tick to the model, then writes the commands the model queued to the
   child, in the order it queued them.
5. `tui/internal/model/model.go` `func (m *Model) Update` holds all the
   decisions and does no IO: keys, frames, the child's exit and the clock
   change the model and queue commands in its outbox.
6. `tui/internal/model/model.go` `func (m *Model) View` builds the screen's
   state, and `tui/internal/view/view.go` `func Render` draws it at the
   terminal's width and height.

## Adding an rpc field a client reads

In this order (ADR 0048 §6, ADR 0052):

1. `crates/trellis-cli/tests/client_contract.json` first: list the field
   under the event, response or trail entry that carries it. A field not
   every client reads yet goes under `rpc.not_every_client` (a trail path,
   `log_json.not_every_client`), with `read_by` naming the clients that do.
   Changes to this file merge one at a time.
2. Then the engine: emit the field. The tests that run the binary fail until
   every listed field is carried:
   `crates/trellis-cli/tests/modes.rs` `fn rpc_stream_carries_every_field_the_desktop_reads`,
   `crates/trellis-cli/tests/modes.rs` `fn log_json_carries_every_trail_field_the_desktop_reads`
   and, for the commands' answers,
   `crates/trellis-cli/tests/modes.rs` `fn rpc_steer_abort_compact_answers_carry_what_the_contract_lists`.
3. Then both clients read it. `desktop/src/state.rs`
   `fn every_field_in_the_rpc_contract_is_read` and
   `tui/internal/frames/frames_test.go` `func TestFramesReadEveryContractField`
   build a record for every event and framing record the contract lists and
   fail when a client's parser drops a field, and `desktop/src/trails.rs`
   `fn every_trail_field_in_the_log_contract_is_read` does the same for
   every `log_json` path; once the clients read a field under
   `not_every_client`, it moves up beside the others.
