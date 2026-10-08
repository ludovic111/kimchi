#!/usr/bin/env bash
# Publishes a kimchi build through lsuite (lsuite DISTRIBUTION.md): copies the files of the draft
# release v<version> that the Release workflow made in ludovic111/kimchi to a new release
# kimchi-v<version> of the private ludovic111/lsuite-builds (same notes, same files: the platform
# files, latest.json, *.sig, SHA256SUMS), then deletes the draft. The lsuite app and kimchi's
# updater get it from there through lsuite.xyz, with the person's account; signatures are
# unchanged, so the server can't alter a build unnoticed.
#   scripts/publish-build.sh <version> [--keep-draft]
# Needs `gh`, signed in with access to both repositories. KIMCHI_REPO and LSUITE_BUILDS_REPO
# override the repositories (for a dry run on forks).
set -euo pipefail

usage() { echo "usage: scripts/publish-build.sh <version> [--keep-draft]" >&2; exit 2; }
[ $# -ge 1 ] || usage
version="${1#v}"
keep_draft=false
[ "${2:-}" = "--keep-draft" ] && keep_draft=true
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+].*)?$ ]] || { echo "Not a version: $1" >&2; usage; }

src_repo="${KIMCHI_REPO:-ludovic111/kimchi}"
dst_repo="${LSUITE_BUILDS_REPO:-ludovic111/lsuite-builds}"
src_tag="v$version"
dst_tag="kimchi-v$version"

command -v gh > /dev/null || { echo "gh (GitHub CLI) is needed: https://cli.github.com" >&2; exit 1; }
command -v jq > /dev/null || { echo "jq is needed." >&2; exit 1; }

# The draft, and only a draft: a published release here would mean the builds went public.
if ! draft=$(gh release view "$src_tag" -R "$src_repo" --json isDraft,body,assets 2> /dev/null); then
  echo "No release $src_tag in $src_repo. Run the Release workflow (push the tag v$version) first." >&2
  exit 1
fi
if [ "$(jq -r .isDraft <<< "$draft")" != true ]; then
  echo "Release $src_tag in $src_repo is published, not a draft. Builds go through lsuite now:" >&2
  echo "turn it back into a draft (gh release edit $src_tag -R $src_repo --draft) and run this again." >&2
  exit 1
fi
names=$(jq -r '.assets[].name' <<< "$draft")
for needed in latest.json SHA256SUMS; do
  grep -qx "$needed" <<< "$names" || { echo "The draft has no $needed: the Release workflow didn't finish." >&2; exit 1; }
done

if gh release view "$dst_tag" -R "$dst_repo" > /dev/null 2>&1; then
  echo "$dst_repo already has $dst_tag; not replacing it. Delete it first to publish again." >&2
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
echo "Downloading the files of $src_repo $src_tag ($(wc -l <<< "$names") files)…"
gh release download "$src_tag" -R "$src_repo" --dir "$work/files"
jq -r .body <<< "$draft" > "$work/notes.md"

# What we copy is what the workflow checked: every file listed in SHA256SUMS matches.
if command -v sha256sum > /dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
(cd "$work/files" && "${sum[@]}" -c SHA256SUMS > /dev/null) || { echo "A file doesn't match SHA256SUMS; nothing was published." >&2; exit 1; }

echo "Creating $dst_repo $dst_tag…"
gh release create "$dst_tag" "$work"/files/* -R "$dst_repo" --title "kimchi $version" --notes-file "$work/notes.md"

# The copy must hold every file before the draft goes.
copied=$(gh release view "$dst_tag" -R "$dst_repo" --json assets -q '.assets[].name' | sort)
if [ "$copied" != "$(sort <<< "$names")" ]; then
  echo "$dst_repo $dst_tag doesn't hold the same files as the draft; the draft is kept." >&2
  diff <(sort <<< "$names") <(echo "$copied") >&2 || true
  exit 1
fi

if $keep_draft; then
  echo "Kept the draft $src_tag in $src_repo (--keep-draft)."
else
  gh release delete "$src_tag" -R "$src_repo" --yes
  echo "Deleted the draft $src_tag in $src_repo."
fi
echo "kimchi $version is out through lsuite: $dst_repo $dst_tag."
