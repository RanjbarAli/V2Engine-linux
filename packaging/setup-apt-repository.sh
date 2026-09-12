#!/usr/bin/env bash
set -euo pipefail

REPOSITORY="RanjbarAli/V2Engine-linux"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
TAG="v${VERSION}"
KEY_HOME="${XDG_CONFIG_HOME:-${HOME}/.config}/v2engine-release"
PRIVATE_KEY="$KEY_HOME/apt-signing-private.asc"

for command_name in gh gpg git; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Missing required command: $command_name" >&2
    echo "On Ubuntu, install GitHub CLI with: sudo apt install gh" >&2
    exit 1
  fi
done

cd "$ROOT"
if [[ -n "$(git status --porcelain)" ]]; then
  echo "Commit the current changes before publishing." >&2
  exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
  gh auth login --web --git-protocol ssh
fi

install -d -m 0700 "$KEY_HOME"
if [[ ! -s "$PRIVATE_KEY" ]]; then
  TEMP_GNUPG="$(mktemp -d)"
  chmod 0700 "$TEMP_GNUPG"
  trap 'rm -rf "$TEMP_GNUPG"' EXIT
  GNUPGHOME="$TEMP_GNUPG" gpg --batch --passphrase '' \
    --quick-generate-key "V2Engine APT Repository <info@aliranjbar.me>" rsa3072 sign 0
  FINGERPRINT="$(GNUPGHOME="$TEMP_GNUPG" gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "fpr" { print $10; exit }')"
  GNUPGHOME="$TEMP_GNUPG" gpg --batch --armor --export-secret-keys "$FINGERPRINT" > "$PRIVATE_KEY"
  chmod 0600 "$PRIVATE_KEY"
fi

gh secret set APT_GPG_PRIVATE_KEY --repo "$REPOSITORY" < "$PRIVATE_KEY"

if gh api "repos/$REPOSITORY/pages" >/dev/null 2>&1; then
  gh api --method PUT "repos/$REPOSITORY/pages" -f build_type=workflow >/dev/null
else
  gh api --method POST "repos/$REPOSITORY/pages" -f build_type=workflow >/dev/null
fi

TAG_POLICY="$(gh api "repos/$REPOSITORY/environments/github-pages/deployment-branch-policies" \
  --jq '.branch_policies[] | select(.name == "v*" and .type == "tag") | .id' 2>/dev/null || true)"
if [[ -z "$TAG_POLICY" ]]; then
  gh api --method POST \
    "repos/$REPOSITORY/environments/github-pages/deployment-branch-policies" \
    -f name='v*' -f type='tag' >/dev/null
fi

EXPECTED_REMOTE="git@github.com:${REPOSITORY}.git"
if git remote get-url origin >/dev/null 2>&1; then
  git remote set-url origin "$EXPECTED_REMOTE"
else
  git remote add origin "$EXPECTED_REMOTE"
fi

if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null; then
  if [[ "$(git rev-list -n1 "$TAG")" != "$(git rev-parse HEAD)" ]]; then
    echo "Tag $TAG already points to another commit; refusing to move it." >&2
    exit 1
  fi
else
  git tag -a "$TAG" -m "V2Engine $VERSION"
fi

git push --set-upstream origin main
git push origin "$TAG"

RUN_ID="$(gh run list --repo "$REPOSITORY" --workflow build.yml --branch "$TAG" --limit 1 --json databaseId --jq '.[0].databaseId // empty')"
if [[ -n "$RUN_ID" ]]; then
  gh run rerun "$RUN_ID" --failed --repo "$REPOSITORY" || true
fi

echo "Published $TAG. GitHub Actions is now building the release and signed APT repository."
echo "Repository URL: https://ranjbarali.github.io/V2Engine-linux/apt"
