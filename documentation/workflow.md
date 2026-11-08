# Workflow

Every change goes through an issue and a pull request, and the git hooks keep
track of both. You commit and push, nothing else. Entering the nix shell
installs the hooks (`thrax hooks` does it by hand).

## The loop

1. Work on `master`.
2. Commit with the issue in the subject: `git commit -m "#351 Fix Windows URIs"`.
   `#new <title>` opens the issue and puts its number in the message:
   `#new Fix Windows URIs` becomes `#352 Fix Windows URIs`.
3. The commit moves to the issue's branch, `352-fix-windows-uris`. `master` goes
   back to where it was, and you are left on the branch. The working tree does
   not change.
4. `git push`. The branch goes up, and its pull request opens, titled with the
   commit subject, `Closes #352` in the body.
5. More commits on the branch need no `#N`, they get the branch's. Each push
   updates the same pull request.
6. Once it merges, `git switch master` fast-forwards `master` and deletes the
   branches whose pull requests merged.

To start something else, `git switch master` and commit there.

## What each hook does

The hooks are one-line shims in `tools/hooks/`; their logic is
`tools/HOOKS.ares`, reached through `thrax hook NAME`.

| Hook | Does |
| --- | --- |
| `pre-commit` | formats the staged `.ares` files; `cargo check` when Rust is staged; the grammar check when `documentation/ares.y` is; refuses a staged `tools/editors` whose Zed extension pins an older grammar commit |
| `commit-msg` | requires `#N` (an open issue) on `master`, opens the issue for `#new`, adds the branch's `#N` elsewhere |
| `post-commit` | moves a `master` commit to its issue's branch; rebuilds what an installed Zed or Helix compiles when it is stale (tools/editors has hooks of its own, see its README) |
| `pre-push` | refuses pushes to `master`, and pushes of a commit pinning an `applications` or `tools/editors` commit not on its remote; opens the pull request of a pushed issue branch |
| `post-checkout`, `post-merge` | arriving on `master`: fast-forward it, delete merged branches; the same rebuild |

Only the message's first line is checked. Merges, reverts and
`fixup!`/`squash!` commits are left alone. Since a subject starts with `#`,
git's comment character is set to `;`.

`pre-commit`, `commit-msg` and `pre-push` refuse to run outside the nix shell
(`nix develop`), where the tools they need are; the other hooks skip.

## Overriding

`--no-verify` skips the checks: `git commit --no-verify` commits on `master`
and the commit stays there, `git push --no-verify` pushes `master`. GitHub
refuses a direct push to `master` from anyone not on the ruleset's bypass list.

Without `thrax` on `PATH` (outside the nix shell) the hooks do nothing.

## The forge

Issues and pull requests go through the `Forge` effect in `tools/FORGE.ares`.
The `github` handler runs `gh`; another host is another handler. The host is
the one `origin` points at, or `git config thrax.forge` (`github`, or
`offline`: issues go unchecked and no pull request opens). When the host does
not answer, a commit's issue goes unchecked rather than the commit failing.

## CI

When `master` goes red, the `report` job opens a `ci-failure` issue, the one to
commit the fix against. Later red runs comment on it, and the next green run
closes it.
