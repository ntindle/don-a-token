# Projects registry

`registry.json` is the manually approved list of donation opportunities the
app offers. It ships as a release artifact next to the desktop installers,
and the app refreshes its copy from the latest release.

## Curation policy

- Additions and removals are maintainer-reviewed. There is no self-serve
  submission yet.
- Every entry needs: a real GitHub repo, a named base branch, sane per-job
  caps, and pull-request attribution enabled.
- Validate before committing: `registry.json` must satisfy `schema.json`
  and pass `cargo test -p don-a-token-core` (the core suite loads the real
  file).
- Bump `updated_at` on every change.

## Entry shape

See `schema.json`. `job.template` names the sandbox template (E2B template
or container image, depending on the runner backend). `job.kind` selects the
prompt pack that turns a project checkout into a bounded Codex task.
