#!/bin/bash
# Publish a Cove release to npm from the artifacts of the "Release binaries" run.
#
#   gh run download <run-id> -R 5h3rd1l/cove -D release-artifacts
#   npm/scripts/publish.sh release-artifacts            # publishes for real
#   DRY_RUN=1 npm/scripts/publish.sh release-artifacts  # checks everything, uploads nothing
#
# Order matters: the four platform packages first (each under its own dist-tag,
# so none of them becomes "latest"), then the launcher, which becomes "latest".
set -euo pipefail
cd "$(dirname "$0")/../.."

dir=${1:?usage: npm/scripts/publish.sh <directory with the downloaded artifacts>}
# Absolute path: npm reads "some/dir/x.tgz" as a GitHub user/repo shorthand.
dir=$(cd "$dir" && pwd)
version=$(node -p "require('./npm/cove-cli/package.json').version")
variants=(linux-x64 linux-arm64 darwin-arm64 darwin-x64)
flags=(--access public)
[ "${DRY_RUN:-}" = 1 ] && flags+=(--dry-run)

# Artifacts download into one folder per platform; flatten what is there.
find "$dir" -name 'cove-cli-*.tgz' -print0 | while IFS= read -r -d '' f; do
  [ "$(dirname "$f")" = "$dir" ] || cp -n "$f" "$dir/"
done

for v in "${variants[@]}"; do
  [ -f "$dir/cove-cli-$version-$v.tgz" ] || { echo "missing $dir/cove-cli-$version-$v.tgz (is this the run for version $version?)" >&2; exit 1; }
done
echo "Publishing cove-cli $version ${DRY_RUN:+(dry run)}"

npm pack ./npm/cove-cli --pack-destination "$dir" >/dev/null
for v in "${variants[@]}"; do
  npm publish "$dir/cove-cli-$version-$v.tgz" "${flags[@]}" --tag "$v"
done
npm publish "$dir/cove-cli-$version.tgz" "${flags[@]}"

echo "Done. Check: npm view cove-cli dist-tags"
