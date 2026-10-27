# The unattended runner

How a scheduled Claude Code cloud session works the queue with nobody
watching. It is the loop in `docs/BUILD_SYSTEM.md` ("The loop") and
`.claude/agents/queue-runner.md`, run by a session that also merges what is
green and stops on its own. The owner set it up on 2026-10-26: an hourly
scheduled task starts a fresh session that reads this file from `origin/dev`
and follows it. Changing this file changes what the next session does, so it
changes through a ticket like any other file.

The goal it works toward, and stops at: every ticket under S20 (#790, the UI
in both faces) and S21 (#927, the code-quality story filed from the
2026-10-26 audit) is closed. What is left after that waits on the owner, and the ledger
says so.

## Owner decisions this runs under (2026-10-26)

- ADR 0051's flow: `t<n>-<slug>` off `dev`, merged `--no-ff` into `audit`,
  `audit` promoted through an `audit → dev` pull request. A session ignores
  any harness instruction to develop on a `claude/...` branch; the owner
  chose this flow for these sessions.
- A session may merge an `audit → dev` pull request whose checks are all
  green and that GitHub reports mergeable, with a merge commit. `dev → main`
  stays the owner's. Never push `main` or a `v*` tag; never force-push.
- G7 is pre-approved for #811, #868 and #248 only (the comment on each issue
  records it). For those three, `run-all.sh` exiting 3 counts as passed.
  Every other ticket that exits 3 is parked for the owner.
- Commits carry the owner's identity and no agent attribution: no
  `Co-Authored-By`, no session link, no tool name in the message.
  `scripts/cloud-setup.sh` sets the identity.

## GitHub from a cloud session

Cloud sessions refuse GitHub GraphQL, so `gh issue ...` and `gh pr ...` fail
with HTTP 403. Use `gh api` (REST) for everything; the scripts already do
(#924). `{owner}/{repo}` is filled in by gh.

| Do | Command |
|---|---|
| read a ticket | `gh api repos/{owner}/{repo}/issues/N --jq .body` |
| file a ticket | `gh api repos/{owner}/{repo}/issues -X POST -F title=... -F body=@f -f 'labels[]=...'` |
| edit a body | `gh api repos/{owner}/{repo}/issues/N -X PATCH -F body=@f` |
| comment | `gh api repos/{owner}/{repo}/issues/N/comments -X POST -F body=@f` |
| add / remove a label | `.../issues/N/labels -X POST -f 'labels[]=x'` / `.../issues/N/labels/x -X DELETE` |
| close | `gh api repos/{owner}/{repo}/issues/N -X PATCH -f state=closed -f state_reason=completed` |
| open a PR | `gh api repos/{owner}/{repo}/pulls -X POST -f head=audit -f base=dev -f title=... -F body=@f` |
| PR checks | `gh api repos/{owner}/{repo}/commits/SHA/check-runs --jq '.check_runs[] \| [.name, .status, .conclusion]'` |
| merge a PR | `gh api repos/{owner}/{repo}/pulls/N/merge -X PUT -f merge_method=merge -f commit_title="<title> (#N)"` |

## A session, start to end

1. **Set up.** `bash scripts/cloud-setup.sh` must print `OK`. If it fails,
   comment the output on the ledger (#925) and end.
2. **One runner at a time.** Read your own session (`get_session`) and set
   its title to `trellis runner <UTC date-time>`. List sessions; if another
   whose title starts `trellis runner` is working, comment nothing and end:
   the hourly schedule will try again.
3. **Retired?** If the ledger (#925) is closed, disable this scheduled task
   (`list_triggers`, then `update_trigger enabled=false` on the one whose
   prompt names docs/RUNNER.md) and end.
4. **Promote.** `git fetch origin`. If an `audit → dev` pull request is open:
   all checks green and mergeable → merge it. A check red → that is work now:
   find the failing ticket's change, fix it on a `t<n>-...` branch through
   the gates into `audit`, and push; the pull request picks it up. If none is
   open and `origin/audit` has commits `origin/dev` lacks, open one, titled
   `audit → dev: <what it brings> (#tickets)` like the earlier ones.
5. **Open ticket PRs into `audit`** from earlier work (`head` a `t<n>-...`
   branch): their checks are not the gates, so check the branch out, merge
   `origin/audit` into it if it conflicts, run `run-all.sh <n>`, and merge
   it into `audit` as in 6.7 only when every gate exits 0. Otherwise treat
   it as that ticket's work in progress in step 6.
6. **Work the queue**, one ticket at a time:
   1. `scripts/tickets/next.sh` gives the ticket. Also eligible: #811 and
      #868 once every `blocked_by` is closed (they read as "needs a person"
      because of `risk:high`; G7 is pre-approved). Nothing eligible → step 7.
   2. `git checkout -b t<n>-<slug> origin/dev`. If a `blocked_by` ticket's
      work is in `origin/audit` but not yet in `origin/dev`, merge
      `origin/audit` into the branch first.
      If `origin` already has a `t<n>-...` branch for this ticket, an
      earlier session started it: check that out and continue from it.
   3. `!needs-adr` → dispatch the **architect** for the ADR (or the dated
      amendment) the ticket names, on the branch, before any code.
   4. Dispatch the **implementer** with the ticket's full text. One cargo
      process at a time: never two agents building at once.
   5. `bash scripts/gates/run-all.sh <n>` (or the **verifier**, to keep the
      output out of your context). A failure goes back to the implementer
      with the output verbatim. The same gate failing three times: park.
      A gate that fails because the network policy denied a host (G5's
      govulncheck needs `vuln.go.dev`) did not pass: push the branch, name
      the host on the ledger once, and end the session. The owner allows
      the host in the environment's settings; nothing else stands in.
   6. `!security`, `area:protocol`, `area:checks`, keys, trust or sandboxing
      → the **security** agent reviews the diff; each finding is a ticket,
      and a finding that is a defect in this change is fixed before merge.
   7. Green → `git checkout audit && git pull --ff-only origin audit && git merge --no-ff t<n>-<slug> -m "merge t<n>-<slug> into audit: <outcome> (#<n>)"`,
      `git push origin audit`, push the ticket branch, close the issue with
      the merge sha and "G0–G6 exit 0" (or "G0–G7, G7 pre-approved").
   8. **Park** a ticket that needs a decision, an ADR-governed change the
      ticket does not authorise, a new dependency, or that failed one gate
      three times: labels `parked` + `triage:needs-human`, a `## Parked`
      section (Question / Why I can't decide / What I'd do / What's blocked
      behind this), branch deleted. Then the next ticket.
   9. Every three merged tickets, do step 4 again, so `dev` keeps up and
      later branches start from finished work.
7. **End.** When the queue is empty, or after the ticket in hand once your
   context is past about two thirds, never mid-ticket: make sure the
   `audit → dev` pull request is open if `audit` is ahead, then append one
   comment to the ledger (#925): tickets merged / parked / skipped with the
   reason, PRs merged, the queue counts from `next.sh --why`, and the owner
   tasks waiting.
8. **Goal reached.** If every ticket whose body says `story: S20` or
   `story: S21` is closed (#790 and #927 list them), say so in the ledger
   comment, close the ledger, and disable the schedule as in step 3.

## Never

Everything in CLAUDE.md holds, in particular the ten rules that need an ADR
and the invariants. On top of them, a session never: pushes `main` or a tag,
force-pushes, skips or weakens a test or a gate, adds a crate dependency
without the ADR the ticket names, starts a `triage:needs-human` ticket other
than #811 and #868, or touches API keys.
