"""Mirror scripts/tickets/next.sh's verdict onto each open issue.

The `ready` label and the triage block's `ready:` / `ready_failures:` lines
are what the board and people read; next.sh (and G0 behind it) is what the
agents obey. This makes the first follow the second, and never the other way:
it writes only when they differ, and changes nothing else in a body.

    scripts/tickets/next.sh --why | python scripts/tickets/sync-ready.py [--dry-run]

It reads next.sh's output on stdin rather than running it: from Windows
Python, `bash` can resolve to WSL's, which has no gh.
"""

import json
import re
import subprocess
import sys

DRY = "--dry-run" in sys.argv


def gh(*args, input=None):
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True,
                          encoding="utf-8", input=input).stdout


def main():
    why = sys.stdin.read()
    if not why.strip():
        sys.exit("no input: pipe `scripts/tickets/next.sh --why` into this")
    verdict = {}
    for line in why.splitlines():
        # next.sh puts \x1f between the reason and the title, because a
        # reason ("parked: waits on ...") can hold ': ' itself.
        m = re.match(r"P\d #(\d+) ([^\x1f]*)\x1f", line)
        if m:
            verdict[int(m.group(1))] = m.group(2)
    # REST, not `gh issue list`: cloud sessions refuse GraphQL (#924). One
    # JSON object per line, pull requests dropped, as issues.sh lists them.
    issues = [json.loads(line) for line in gh(
        "api", "--paginate", "repos/{owner}/{repo}/issues?state=open&per_page=100",
        "--jq", ".[] | select(.pull_request | not) | {number, labels, body: (.body // \"\")}",
    ).splitlines() if line]
    changed = 0
    for i in issues:
        n = i["number"]
        if n not in verdict:
            continue
        reason = verdict[n]
        ready = reason == "ELIGIBLE"
        has_label = any(l["name"] == "ready" for l in i["labels"])
        body = i["body"]
        failures = "[]" if ready else json.dumps([reason])
        # Issue bodies are CRLF; `.*` would eat the \r and leave a bare \n.
        new = re.sub(r"(?m)^ready: [^\r\n]*", f"ready: {'true' if ready else 'false'}", body, count=1)
        new = re.sub(r"(?m)^ready_failures: [^\r\n]*", f"ready_failures: {failures}", new, count=1)
        if ready == has_label and new == body:
            continue
        changed += 1
        print(f"#{n}: ready={ready} ({reason})")
        if DRY:
            continue
        if ready != has_label:
            if ready:
                gh("api", "-X", "POST", f"repos/{{owner}}/{{repo}}/issues/{n}/labels", "-f", "labels[]=ready")
            else:
                gh("api", "-X", "DELETE", f"repos/{{owner}}/{{repo}}/issues/{n}/labels/ready")
        if new != body:
            gh("api", "-X", "PATCH", f"repos/{{owner}}/{{repo}}/issues/{n}", "-F", "body=@-", input=new)
    print(f"{changed} issue(s) {'would change' if DRY else 'changed'}")


if __name__ == "__main__":
    main()
