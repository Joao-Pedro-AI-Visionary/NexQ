<!--
Thanks for contributing to NexQ! Please read CONTRIBUTING.md if this is your first PR.
Give the PR a Conventional Commit title, e.g. "fix(overlay): keep position between sessions".
-->

## Summary

<!-- What does this PR change, and why? -->

Closes #

## Type of change

- [ ] Bug fix (`fix`)
- [ ] New feature (`feat`)
- [ ] Breaking change (`feat!` / `BREAKING CHANGE:`)
- [ ] Performance (`perf`)
- [ ] Refactor, tests, docs, CI or chore

## How was this tested?

<!-- Describe manual testing: OS, providers, meeting app, steps. -->

- [ ] Windows
- [ ] macOS (Apple Silicon)

## Screenshots / recordings

<!-- Required for UI changes: before and after. Delete if not applicable. -->

## Checklist

- [ ] `npx tsc --noEmit` passes
- [ ] `npm test` passes
- [ ] `cargo test --lib` passes (in `src-tauri/`), if Rust changed
- [ ] Commits follow [Conventional Commits](https://www.conventionalcommits.org/)
- [ ] `src/lib/types.ts` matches any changed Rust structs, and new commands have wrappers in `src/lib/ipc.ts`
- [ ] Platform-specific code is gated with `#[cfg(target_os = "...")]` and the other platform still compiles
- [ ] User-facing docs updated (`docs/user-guide/`, README) if behavior changed
- [ ] I did **not** bump versions, edit `CHANGELOG.md`, or create tags (maintainers handle releases)
