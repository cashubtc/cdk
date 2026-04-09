# Contributing to CDK

How to participate in the **Cashu Development Kit**: where to talk, how to land changes, and what reviewers look for. Technical setup (Nix, databases, CI, profiling) stays in **[DEVELOPMENT.md](DEVELOPMENT.md)** so this page stays about **people and process**.

## What CDK is

**CDK** is a Rust workspace for [Cashu](https://github.com/cashubtc) wallets and mints: protocol pieces in **`cashu`**, the main SDK in **`cdk`**, storage and Lightning crates, **`cdk-cli`**, **`cdk-mintd`**, HTTP layers, FFI bindings, and more. The **[README](README.md)** lists crates and implemented NUTs.

Everyone can propose changes. **[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)** applies to issues, Matrix, and PRs.

## Quick Project Tour

New to the codebase? Here's the 30-second orientation:

```text
cdk/
├── crates/                         # 24 crates - full map in README
│   ├── cashu/  cdk/  cdk-common/   # Protocol, SDK, shared traits
│   ├── cdk-cli/  cdk-mintd/        # Wallet CLI · mint daemon
│   ├── cdk-sqlite/ … + cdk-sql-common/   # Storage (+ shared SQL)
│   ├── cdk-axum/  cdk-http-client/ # Mint HTTP · wallet client
│   └── cdk-cln/ … cdk-fake-wallet # Lightning backends (+ test fake wallet)
├── CONTRIBUTING.md    # ← You are here (people & process)
├── DEVELOPMENT.md     # Setup, architecture, CI, testing
└── CODE_STYLE.md      # Rust style rules
```

## Start here (even before a patch)

**Reading and running tests teaches the codebase fast.** Commenting on PRs, trying a branch locally, or reproducing a bug report helps maintainers and future you.

Before you send a patch:

1. Skim **[DEVELOPMENT.md](DEVELOPMENT.md)** for `cargo` / `just` / Nix flows so you are not guessing flags.
2. **Build and test** the crates you touch (`cargo test -p …`, `cargo clippy`, or `just final-check` when you use Nix; see **[DEVELOPMENT.md](DEVELOPMENT.md)**).
3. For protocol-heavy work, skim the relevant **[NUT](https://github.com/cashubtc/nuts)** and existing **`cashu`** / **`cdk`** types so your change stays aligned with the spec.

## Your First PR - Quick Checklist

Never contributed to Rust before? This is the happy path:

- **Fork** the repo on GitHub and **clone** your fork locally
- **Create a branch**: `git checkout -b fix/descriptive-name` or `feat/...`
- **Make your changes** (code + tests when behavior changes)
- **Format & lint**: `cargo fmt` and `cargo clippy -- -D warnings`
- **Test locally**: `cargo test -p crate-you-changed`
- **Push** to your fork: `git push origin your-branch-name`
- **Open a PR** to `cashubtc/cdk` targeting `main`
- **Respond to review feedback** in PR comments
- **Wait for CI** to go green, then a maintainer merges
- 🎉 **Celebrate!** Your code is now part of CDK

**Stuck on any step?** Ask in the [Matrix #dev channel](https://matrix.to/#/#dev:matrix.cashu.space) or comment on your PR.

**Detailed walkthrough** of each step: See [End-to-end workflow](CONTRIBUTING.md#end-to-end-workflow) below.

---

## Pick something to work on

**GitHub issues** are the main queue. Labels help filter:

| Label (examples)     | Rough meaning                        |
| -------------------- | ------------------------------------ |
| **good first issue** | Smaller surface; less context needed |
| **help wanted**      | Maintainers would like help          |
| **documentation**    | Docs, examples, clarifications       |

Issues can go stale if you are unsure, **leave a short comment** (“looking at this next week”) so others do not duplicate work. You do **not** need permission to start; courtesy comments reduce collisions.

**Ideas without an issue:** open an issue for larger design shifts; **typos and tiny doc fixes** can go straight to a PR.

---

## Common First Contributions

**Good entry points if you're new to the codebase:**

- **Fix typos or improve error messages** - User-facing strings and `thiserror` text across `crates/*`; low-risk, builds familiarity
- **Add examples to rustdoc comments** - Public API in **`cashu`**, **`cdk`**, **`cdk-common`**, and other crates often lacks `/// # Examples`
- **Add tests for untested code paths** - In the crate you change (`crates/.../src/`) or in **`cdk-integration-tests`** when behavior crosses crates; `rg '#\[test\]' crates/` helps find patterns
- **Improve CLI help text** - **`cdk-cli`** (`crates/cdk-cli`); `--help` and subcommand descriptions can always be clearer
- **Update outdated documentation** - Root **`README.md`**, **`DEVELOPMENT.md`**, per-crate **`README.md`**, or rustdoc when it drifts from the code

**Harder first PRs** (consider after you've landed one or two smaller changes):

- Large refactors across multiple crates
- New cryptographic primitives or proof logic
- Protocol changes (requires deep NUT spec knowledge)
- Storage backend rewrites (high risk of data corruption bugs)

**When in doubt**, pick something from the **`good first issue`** label or ask in Matrix what would help right now.

---

## Where we talk

| Channel                                                     | Use for                                                        |
| ----------------------------------------------------------- | -------------------------------------------------------------- |
| [Matrix #dev](https://matrix.to/#/#dev:matrix.cashu.space)  | Quick questions, coordination, informal design chat            |
| **[GitHub Issues](https://github.com/cashubtc/cdk/issues)** | Bugs, feature proposals, decisions that should stay searchable |

**Development meeting:** A **monthly** CDK dev call (voice/video) is the main synchronous touchpoint. Join at **[meet.fulmo.org/cdk-dev](https://meet.fulmo.org/cdk-dev)**. Agendas land as PRs under [`meetings/`](meetings/)-check the latest file for the next **date and time (UTC)**.

Keep technical debate **in the open** (issue or PR) when it helps the next contributor.

---

## End-to-end workflow

Single **GitHub repo**: [cashubtc/cdk](https://github.com/cashubtc/cdk). Everything lands via **pull requests** against **`main`**.

1. **Fork** and **clone** your fork ([GitHub fork docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/fork-a-repo)).
2. Add **`upstream`**: `git remote add upstream https://github.com/cashubtc/cdk.git`
3. Create a **topic branch** from up-to-date `main`: `git checkout -b fix/short-topic` or `feat/…`
4. **Implement** with **tests** where behavior changes (see [Testing](CONTRIBUTING.md#what-ready-to-merge-usually-means)).
5. Run **`cargo fmt`**, **`cargo clippy`** (workspace rules use **`-D warnings`** in CI), or **`just final-check`** when available.
6. **Push** to **your fork** and open a **PR** to **`cashubtc/cdk`** targeting **`main`**.

You should **understand** what you submit and **run** the checks that apply to your change. If something is experimental, say so in the PR description.

---

## Commits

**Prefer small, logical commits**: one coherent idea per commit when practical. That makes review and `git bisect` easier.

- **Do not** mix large **rustfmt-only** or **move-only** edits with behavior changes in the same commit unless maintainers ask for a single squashed commit.
- **Pair code and tests**: when behavior changes, update tests **in the same commit** when possible.
- **Conventional Commits** style (type, optional scope, subject):
  ```text
  feat(cdk-cli): add balance subcommand
  fix(cdk-sqlite): handle empty migration set
  docs(cdk): clarify WalletBuilder example
  ```
- **Body**: explain _why_ when the diff does not speak for itself. Link issues in the footer: `Fixes #123`, `Closes #456`.

**Avoid `@username` mentions inside commit messages**: they generate noise when history is copied or mirrored.

Rust-specific habits that match this repo: respect **`unwrap_used = deny`** in production code, prefer **`?`**, and add **rustdoc** (`///`) for new public items ([AGENTS.md](AGENTS.md), [CODE_STYLE.md](CODE_STYLE.md)).

---

## Pull requests

**Title** - Mirror the commit style: `type(scope): subject`. **Scope** is often a crate or area:

| Scope examples                                 | Typical use               |
| ---------------------------------------------- | ------------------------- |
| `cashu`, `cdk`, `cdk-common`                   | Core library and protocol |
| `cdk-cli`, `cdk-mintd`                         | Binaries and UX           |
| `cdk-sqlite`, `cdk-postgres`, `cdk-sql-common` | Storage                   |
| `cdk-axum`, `cdk-http-client`                  | HTTP server / client      |
| `cdk-cln`, `cdk-lnd`, …                        | Lightning backends        |
| `ci`, `docs`                                   | Infra and top-level docs  |

**Description** should answer:

- **What** changed (user-visible or API).
- **Why** (problem, tradeoff, link to issue).
- **How you tested** (commands, or “covered by `cargo test -p …`”).

Link related issues with `Fixes`, `Closes`, or `Related to`. If you need someone’s eyes, **@mention in a follow-up comment** rather than stuffing the opening post-merges and forks can resurface descriptions.

---

## Drafts, WIP, and early feedback

Use **GitHub Draft PRs** or prefix the title with **`[WIP]`** when the branch is not ready to merge. **Checklists** in the description help reviewers see what is left. Switch to “Ready for review” when CI-relevant work is done.

---

## While your PR is open

- **Reply** to review threads even a “done” or “won’t change because …” helps.
- **Push new commits** (or amend locally if you prefer) and **re-run** the checks you care about.
- **Stale feedback** with no response may lead to closing; reopening is fine when you have bandwidth.

---

## Squashing commits

Maintainers may ask you to **squash** noisy fixup chains into one or a few clean commits before merge.

```bash
git checkout your-branch
git rebase -i HEAD~n   # n = number of commits to fold
# mark fixups as squash/fixup, save
git push --force-with-lease origin your-branch
```

The **resulting message** should read as one story, not a dump of interim titles.

---

## Keeping up with `main` (rebase or merge)

When **`main`** advances and your branch lags, update before merge or final review:

**Rebase** (linear history on top of current upstream `main`):

```bash
git fetch upstream
git checkout your-branch          # skip if you are already on it
git rebase upstream/main
# conflicts: fix → git add … → git rebase --continue
git push --force-with-lease origin your-branch
```

Your local `main` does not need to be current-`git fetch upstream` updates `upstream/main`, which is what you rebase onto.

**Merge** instead of rebase if you want a merge commit on your branch (no rewrite, so a normal **`git push`**-no **`--force`**):

```bash
git fetch upstream
git checkout your-branch
git merge upstream/main
# resolve conflicts if any, then:
git push origin your-branch
```

---

## What makes a strong PR in this repo

- **One main intent** per PR: a bugfix, a feature, a doc refresh, or a focused refactor-not all at once.
- **Review-sized diffs**: large mechanical edits are harder to land than a sequence of smaller PRs.
- **Clear scope**: say which crates and features are affected (`cdk`’s `wallet` / `mint` flags, storage backends, etc.).

---

## Features, refactors, and risk

**New features** carry ongoing cost (bugs, API surface, docs). If you propose something sizable, **say whether you can help maintain it** after merge; orphaned features are harder to justify.

**Refactors** should usually **not change behavior**; separate them from behavior fixes. **Wide refactors** are easier for people who already know the module graph-if you are new, start with **narrow** changes and ask on Matrix when unsure.

**Tiny or unclear refactors** may be closed to keep reviewer load manageable-that is about **focus**, not you personally.

---

## What “ready to merge” usually means

> **Before you open a PR, run these locally:**
>
> ```bash
> cargo test -p crate-you-changed  # Tests pass
> cargo fmt                         # Code formatted
> cargo clippy -- -D warnings       # No lint warnings
> ```

Maintainers weigh **fit with Cashu/CDK**, **review quality**, and **CI health**. Expectations:

- **Tests** - New logic covered where practical; integration paths when behavior crosses crates ([DEVELOPMENT.md - Testing](DEVELOPMENT.md#4-testing-strategy)).
- **Lint / style** - `cargo fmt`, `cargo clippy -- -D warnings`, `typos` (see CI).
- **Docs** - `rustdoc` for public API changes; **CHANGELOG** for user-visible changes ([CHANGELOG.md](CHANGELOG.md)).
- **Green CI** - See [DEVELOPMENT.md - CI & pipeline](DEVELOPMENT.md#13-cicd-pipeline).

Protocol-sensitive areas (cryptography, proofs, key handling) deserve **extra care and review time**-not because of “consensus” in the Bitcoin sense, but because mistakes cost real users.

---

## Stable branches and backports

**Bugfixes and selected changes** often flow from `main` to **version branches** with **`backport v0.x.x`** labels on the merged PR. Details: **[DEVELOPMENT.md - Release & backporting](DEVELOPMENT.md#12-release-process)**.

---

## License

By contributing, you agree your work is included under the project’s **[MIT license](LICENSE)** unless a file header says otherwise. Third-party code must keep its license and attribution.

---

## Found a Security Issue?

**Do NOT** open a public GitHub issue.

Security vulnerabilities should be reported privately. See **[SECURITY.md](SECURITY.md)** for our responsible disclosure process.

Cryptographic bugs, proof verification bypasses, and anything that could let an attacker steal funds or forge tokens deserve private reporting.

---

## Further reading

| Doc                                          | Purpose                                                  |
| -------------------------------------------- | -------------------------------------------------------- |
| **[DEVELOPMENT.md](DEVELOPMENT.md)**         | Architecture, crates, Nix, tests, CI, Docker, migrations |
| **[CODE_STYLE.md](CODE_STYLE.md)**           | Style rules                                              |
| **[AGENTS.md](AGENTS.md)**                   | Workspace commands and conventions                       |
| **[REGTEST_GUIDE.md](REGTEST_GUIDE.md)**     | Local Bitcoin + Lightning + mint testing                 |
| **[SECURITY.md](SECURITY.md)**               | Responsible disclosure                                   |
| **[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)** | Community expectations for issues, chat, and PRs         |
| **[meetings/](meetings/)**                   | Dev call agendas (schedule and UTC time in latest file)  |

Thank you for helping improve CDK.
