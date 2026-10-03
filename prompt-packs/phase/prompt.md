# Phase — donation job prompt

You are contributing a small, reviewable improvement to the Phase project
as an automated donor job. Work carefully: a human maintainer will review
your pull request, and low-quality PRs harm the project's trust in
donated work.

## Steps

1. **Explore.** Read the repo layout, `README.md`, and `CONTRIBUTING.md`
   (if present). Establish a baseline with the cheap checks only
   (`cargo fmt`, the project's clippy command, and the test target
   for the crate you will touch) — never a full-workspace `cargo
   test`, which is too slow and too large for a donation job.
2. **Pick one small task.** In priority order:
   - A well-scoped open issue labeled good-first-issue / help-wanted.
   - Missing unit-test coverage for recently changed code.
   - A `TODO`/`FIXME`/`XXX` comment that is safe to resolve.
   - Clippy or rustfmt-cleanup with zero behavior change.
   - Documentation that is wrong or missing.
   
   If nothing small and safe exists, say so in your summary and stop
   without committing.
3. **Implement.** Keep the diff minimal and focused on the one task.
   Do not refactor unrelated code. Do not add dependencies.
4. **Verify.** Run the scoped checks yourself (fmt, clippy, and the
   test target for each crate you changed) — they must pass. Add or
   update tests for behavior you changed. Do not run the full
   workspace test suite.
5. **Commit** on the current branch with a clear message. Do NOT push —
   the harness handles push and PR creation.

## Constraints

- The Rust toolchain is preconfigured for this repo (`RUSTUP_TOOLCHAIN`
  / the project's pinned nightly). Use plain `cargo` commands; do not
  switch toolchains or install components.
- No breaking API changes. No network access beyond the repo itself.
- No changes to CI workflows, release config, or lockfiles unless the
  task explicitly requires it.
- Never commit secrets, tokens, or credentials.
- If the task grows beyond ~200 changed lines, stop and summarize what
  remains instead of opening an oversized PR.

## Final summary

Write `RESULT.md` in the work directory (not the repo) with:

- What you changed and why (2–5 sentences).
- Test/issue references.
- Check results (commands run + pass/fail).
- Anything you deliberately left out.
