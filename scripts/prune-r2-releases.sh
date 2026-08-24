#!/usr/bin/env bash
#
# Prune old NovaProxy release artifacts from the R2 (S3-compatible) bucket,
# keeping only the newest KEEP versions. Run after a release publishes.
#
# Why keep 2 and not 1: the previous version is what a user rolls back to when
# a release turns out bad, and it is what a future updater would delta against.
# Anything older is dead weight in a bucket that pays per GB.
#
# Only versioned artifacts are eligible for deletion — the version comes from
# Tauri's bundle names:
#   NovaProxy_<ver>_aarch64.dmg / _x64.dmg        (macOS)
#   NovaProxy_<ver>_x64-setup.exe                 (Windows NSIS)
#   NovaProxy_<ver>_x64_en-US.msi                 (Windows MSI)
# Keys with no extractable X.Y.Z are NEVER touched. That is the safety net: it
# is what protects an updater manifest (latest.json) or any hand-uploaded file
# from being swept up by a version-based rule.
#
# Env (set by CI):
#   R2_RELEASES_ACCOUNT_ID, R2_RELEASES_BUCKET
#   AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY
# Optional:
#   PREFIX   restrict to a key prefix (default "", the whole bucket)
#   KEEP     number of versions to retain (default 2)
#   DRY_RUN  "1" to print deletions without executing (default 0)
#
set -euo pipefail

KEEP="${KEEP:-2}"
DRY_RUN="${DRY_RUN:-0}"
PREFIX="${PREFIX:-}"
ENDPOINT="https://${R2_RELEASES_ACCOUNT_ID}.r2.cloudflarestorage.com"
export AWS_DEFAULT_REGION="auto"

aws_s3() { aws s3api "$@" --endpoint-url "$ENDPOINT"; }

echo "Listing objects in bucket '${R2_RELEASES_BUCKET}' (prefix '${PREFIX}')…"
mapfile -t KEYS < <(
  aws_s3 list-objects-v2 --bucket "$R2_RELEASES_BUCKET" --prefix "$PREFIX" \
    --query 'Contents[].Key' --output text | tr '\t' '\n' | sed '/^$/d'
)
echo "Found ${#KEYS[@]} object(s)."

if [ "${#KEYS[@]}" -eq 0 ]; then
  echo "Empty bucket. Nothing to prune."
  exit 0
fi

# Distinct versions present, newest first (semver sort via sort -V).
mapfile -t VERSIONS < <(
  printf '%s\n' "${KEYS[@]}" \
    | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' \
    | sort -uV -r
)

if [ "${#VERSIONS[@]}" -le "$KEEP" ]; then
  echo "Only ${#VERSIONS[@]} version(s) present (<= KEEP=${KEEP}). Nothing to prune."
  exit 0
fi

KEEP_VERSIONS=("${VERSIONS[@]:0:$KEEP}")
PRUNE_VERSIONS=("${VERSIONS[@]:$KEEP}")
echo "Keeping: ${KEEP_VERSIONS[*]}"
echo "Pruning: ${PRUNE_VERSIONS[*]}"

in_keep() {
  local v
  for v in "${KEEP_VERSIONS[@]}"; do [ "$v" = "$1" ] && return 0; done
  return 1
}

DELETE_KEYS=()
for key in "${KEYS[@]}"; do
  ver="$(printf '%s' "$key" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
  # No version in the key (manifests, anything hand-uploaded) → never delete.
  [ -z "$ver" ] && continue
  in_keep "$ver" || DELETE_KEYS+=("$key")
done

if [ "${#DELETE_KEYS[@]}" -eq 0 ]; then
  echo "No stale keys to delete."
  exit 0
fi

echo "Will delete ${#DELETE_KEYS[@]} key(s):"
printf '  %s\n' "${DELETE_KEYS[@]}"

if [ "$DRY_RUN" = "1" ]; then
  echo "DRY_RUN=1 — no objects deleted."
  exit 0
fi

# delete-objects takes a batch of up to 1000 keys.
OBJECTS_JSON="$(printf '%s\n' "${DELETE_KEYS[@]}" \
  | jq -R '{Key: .}' | jq -s '{Objects: ., Quiet: false}')"

aws_s3 delete-objects --bucket "$R2_RELEASES_BUCKET" --delete "$OBJECTS_JSON"
echo "Pruned ${#DELETE_KEYS[@]} object(s)."
