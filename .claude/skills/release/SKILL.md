---
name: release
description: Cut a NovaProxy release — bump the version, land it through a PR, tag, and verify the published artifacts. Use when asked to release, ship a version, bump the version, or cut a tag.
---

# Releasing NovaProxy

A release is a **tag push**. `.github/workflows/release.yml` runs on `v*.*.*` and
does everything else: builds the matrix, signs, uploads to the GitHub Release and
to R2, then undrafts. Nothing is released by merging to `main`.

`workflow_dispatch` on the same workflow is the dry run — it builds and signs
every target, keeps the installers as run artifacts, and touches neither GitHub
Releases nor R2. Use it when the change is to the packaging itself.

## 1. Decide the number

Pre-1.0, so: minor for anything a user would notice, patch for fixes alone.
Ask rather than guess when the answer changes what ships.

Read what is actually going out first — the tag is not always where you think:

```sh
git checkout main && git pull --ff-only
git log --oneline "$(git describe --tags --abbrev=0)"..main
```

## 2. Bump five files

Three manifests hold the version, and two lockfiles carry it:

| File | How |
|---|---|
| `package.json` | edit |
| `src-tauri/tauri.conf.json` | edit |
| `Cargo.toml` (workspace `[workspace.package]`) | edit |
| `Cargo.lock` | `cargo metadata --format-version 1 >/dev/null` |
| `package-lock.json` | `npm install --package-lock-only` |

The crates all use `version.workspace = true`, so the workspace `Cargo.toml` is
the only Rust edit — but `Cargo.lock` pins each of the five crates by version and
must be regenerated, not hand-edited.

Confirm the diff is version lines and nothing else:

```sh
git diff --stat
git diff Cargo.lock package-lock.json | grep '^[+-].*version'
```

## 3. Verify locally before the PR

```sh
npm test                    # 138+ tests, ~0.5s
cargo build -p novaproxy    # the tauri crate compiles at the new version
```

`npm run app` needs a helper binary the dev command does not build:

```sh
cargo build --release -p nova-helper    # once; beforeDevCommand skips it
```

## 4. Land the bump through a PR

Never commit the bump straight to `main` — the repo merges everything through
PRs, and the bump commit is what the tag will point at.

```sh
git checkout -b chore/release-<version>
git commit -am "chore: <version>"
git push -u origin chore/release-<version>
gh pr create --base main --title "chore: <version>"
```

The PR body should say what the release contains over the *previous tag*, not
over the previous PR.

## 5. Tag from merged main

```sh
git checkout main && git pull --ff-only
grep '"version"' src-tauri/tauri.conf.json     # must match the tag you are about to cut
git tag -a v<version> -m "NovaProxy <version>

<one or two lines on what changed>"
git push origin v<version>
```

**The tag must be cut after the bump is merged.** `v0.1.1` was tagged at a commit
where `tauri.conf.json` still read `0.1.0`, so those installers report the wrong
version in-app and nothing catches it after the fact. The `preflight` job now
compares the tag against both `tauri.conf.json` and `package.json` and fails the
run, so a mistake costs a re-tag rather than a bad release.

## 6. Watch the run

```sh
gh run list --workflow=Release --limit 3
gh run view <run-id> --json status,conclusion,jobs
```

Four jobs, in order:

1. **Check tag matches version** — fails in under a minute if step 5 was wrong.
2. **macOS (Apple silicon)** and **Windows (x64)** — build, sign, upload to a
   *draft* release.
3. **Mirror to R2 and prune** — flattens the installers, writes `latest.yml` and
   `latest.json`, uploads to R2, attaches the manifests, then prunes R2 down to
   the newest 2 versions.
4. **Publish release** — flips the draft. Separate job on purpose: a partial
   matrix or a failed mirror leaves the release invisible rather than
   half-published.

Roughly 15 minutes end to end.

## 7. Verify the artifacts, not the conclusion

A green run does not mean a complete release. The updater config is injected only
when `TAURI_UPDATER_PUBKEY` and `R2_PUBLIC_BASE_URL` are both present; missing
either emits a `::warning::` and **builds on**, producing a release that cannot
self-update. Check what actually landed:

```sh
gh release view v<version> --json isDraft,assets -q '.isDraft, .assets[].name'
```

Expect all of:

- `NovaProxy_<version>_aarch64.dmg`
- `NovaProxy_<version>_x64-setup.exe` and `NovaProxy_<version>_x64_en-US.msi`
- a `.sig` next to each updater artifact
- `NovaProxy_<version>_aarch64.app.tar.gz` — the macOS updater bundle
- `latest.json` (updater manifest) and `latest.yml`
- `isDraft: false`

No `.sig` files or no `latest.json` means the updater secrets were missing. The
build is still installable; it just cannot update itself, and the *next* release
will not reach these users automatically.

## 8. If the run fails

Fix forward on a branch, merge, then move the tag:

```sh
git tag -d v<version> && git push origin :refs/tags/v<version>
gh release delete v<version> --yes          # only if a draft release was created
# ...merge the fix...
git tag -a v<version> -m "..." && git push origin v<version>
```

Only do this while the release is still a draft. Once `publish` has run, people
may have the installer — cut a new patch version instead.

## Things that look broken and are not

- **`gh secret list` prints nothing.** The signing, R2 and updater secrets live at
  the org level and in the `Production` environment; the repo has none of its
  own. Confirm by looking at whether the last release produced `.sig` files, not
  by listing secrets.
- **The release is missing right after the matrix finishes.** It is a draft until
  the `publish` job runs.
- **R2 only holds two versions.** `scripts/prune-r2-releases.sh` runs with
  `KEEP=2` on every release; the previous version stays as a rollback target and
  older ones are deleted.
