# These files do not compile, and that is the test

Three attempts to forge a `Verdict` from outside `trellis-checks`, each with
its committed compiler output in the `.stderr` beside it. `tests/seal.rs`
drives them through `trybuild` and fails if any of them *succeeds*.

So your editor will show errors here. It is right: these are compile failures.
They are also the proof that the grounding contract is structural rather than
advisory — if they ever stop failing, a model's claim of success could be
turned into a passing verdict, which is the one thing this project refuses.

`.vscode/settings.json` excludes this directory from rust-analyzer for that
reason. Anything else is a real problem.

Run them with:

    cargo test -p trellis-checks --test seal

If a toolchain upgrade changes the wording of a compiler error, regenerate the
expected output with `TRYBUILD=overwrite cargo test -p trellis-checks --test
seal` and **read the diff** — a message that no longer mentions privacy means
the seal changed, not the formatting.
