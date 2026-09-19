#!/usr/bin/env bash
# Verify the standalone npm SDK package (crw-sdk):
#   1. Existence of crw-sdk@version
#   2. Install + dual-format import smoke (CommonJS require + ESM import)
#
# Usage: verify_npm_sdk.sh <version>
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

v="${1:?version required}"
pkg="crw-sdk" # single source: keep in sync with sdks/typescript/package.json "name"

# 1. Existence — poll, since npm registry propagation can lag after publish
# (otherwise this false-fails a successful release). The window matches
# verify_npm.sh, which was widened to five minutes after 0.34.0 measured more
# than a minute. This script kept the old 60 s and so false-failed both 0.35.1
# and 0.36.0: npm logged `+ crw-sdk@<version>` and said the package "may take a
# few minutes to become available", then the check gave up at 60 s while the
# release itself was fine.
actual="MISSING"
for _ in $(seq 1 30); do
  actual=$(npm view "$pkg@$v" version 2>/dev/null || echo "MISSING")
  [ "$actual" = "$v" ] && break
  sleep 10
done
[ "$actual" = "$v" ] || die "$pkg@$v not on npm after retries (got: $actual)"

# 2. Install + dual-format import smoke, with the same propagation allowance:
# a package that `npm view` already sees can still be a few seconds from
# installable.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
installed=0
for _ in $(seq 1 6); do
  if (cd "$tmp" && rm -rf node_modules package-lock.json && npm init -y >/dev/null \
      && npm install --silent "$pkg@$v" >/dev/null 2>&1); then
    installed=1
    break
  fi
  sleep 10
done
[ "$installed" = 1 ] || die "npm install $pkg@$v failed"
(cd "$tmp" && node -e "const {CrwClient}=require('$pkg'); if(typeof CrwClient!=='function') process.exit(1)") \
  || die "CJS require smoke failed for $pkg@$v"
(cd "$tmp" && node --input-type=module -e "import {CrwClient} from '$pkg'; if(typeof CrwClient!=='function') process.exit(1)") \
  || die "ESM import smoke failed for $pkg@$v"

notice "verify_npm_sdk OK: $pkg@$v"
