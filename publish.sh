set -euo pipefail

TAG="v0.1.0" # set to the tag you are releasing
VERSION="$(node scripts/npm/assert-release-version.mjs --tag "$TAG" --cargo-toml Cargo.toml)"

node scripts/npm/build-packages.mjs \
  --version "$VERSION" \
  --artifacts-dir dist \
  --output-dir dist/npm

for package_dir in $(find dist/npm/platform -mindepth 1 -maxdepth 1 -type d | sort); do
  npm publish "$package_dir" --access public
done

npm publish dist/npm/base --access public
