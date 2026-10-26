# Plan

The agent's plan as a checklist you can edit, and its latest reasoning as text you can rewrite. Steps are `✓` done (struck through), `●` current (on `signal-soft`), `○` to do; click a step to edit its text, use the hover controls to make it current, skip it or remove it, and `+ add a step` to insert one. Below, "the agent's reasoning · turn N" shows the plan text the agent is working from; `Edit` turns it into a textarea and `Replace and steer` sends your version. Every change is delivered to the agent as a steer before its next model call and recorded in the trail as yours — the component never pretends to edit the model's mind directly.

Consumer provides: `steps`, `reasoning` and `turn` from the latest plan event, `onChange(steps)` and `onSteer(text)` wired to `steer`. Keep the list short: a plan over eight steps is a sign the task should have been split. In the terminal the same list renders with `[x] [>] [ ]` and the reasoning under a rule.
