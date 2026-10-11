#!/bin/sh
# Release omabubble and omabubble-bridge together as one version.
#
#   scripts/release.sh 0.2.0
#
# 1. Checks: clean main, in step with GitHub, version not tagged or published,
#    notes under "## Unreleased" in CHANGELOG.md, tests pass.
# 2. Sets the version in Cargo.toml, dates the changelog section, commits
#    "Release 0.2.0" and tags v0.2.0.
# 3. Builds omabubble here, and omabubble-bridge on the Mac, where it's signed with the
#    Mac's signing identity so Full Disk Access survives the update.
# 4. Publishes both with artifact-push to artifacts.example.com
#    (omabubble/v0.2.0/ and omabubble-bridge/v0.2.0/, with release notes), then pushes
#    the commit and tag to GitHub.
#
# Published versions are immutable: fix a bad release with a new patch version.
#
# Settings come from the environment or from local.env in the repo root (not
# committed; see local.env.example): OMABUBBLE_MAC and OMABUBBLE_MAC_PORT, the Mac to
# build the bridge on, reached with your normal SSH key, not the bridge-only
# one; ARTIFACTS_URL, the artifacts site.
set -eu
cd "$(dirname "$0")/.."
[ ! -f local.env ] || . ./local.env

MAC=${OMABUBBLE_MAC:-you@your-mac}
MAC_PORT=${OMABUBBLE_MAC_PORT:-2233}
# The Mac's copy of the source, relative to its home: a build mirror that each
# release overwrites (rsync --delete), so it lives with other caches.
MAC_DIR=${OMABUBBLE_MAC_DIR:-Library/Caches/omabubble/release}
MAC_MISE=/opt/local/bin/mise
SITE=${ARTIFACTS_URL:-https://artifacts.example.com}

die() {
    echo "release: $*" >&2
    exit 1
}
mac() { ssh -p "$MAC_PORT" -o BatchMode=yes "$MAC" "$@"; }

[ $# = 1 ] || die "usage: scripts/release.sh <version>, e.g. 0.2.0"
version=${1#v}
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "version must look like 0.2.0"
tag=v$version

# ---------------------------------------------------------------- checks
[ "$(git branch --show-current)" = main ] || die "release from main"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "commit or stash your changes first"
git fetch -q origin main
[ "$(git rev-list --count HEAD..origin/main)" = 0 ] || die "main is behind origin/main; pull first"
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "$tag is already tagged"
for tool in omabubble omabubble-bridge; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "$SITE/$tool/$tag/")
    [ "$code" = 404 ] || die "$tool $tag is already on $SITE (HTTP $code)"
done
command -v artifact-push >/dev/null || die "artifact-push isn't installed"
notes=$(awk '/^## Unreleased/ {f = 1; next} /^## / {f = 0} f' CHANGELOG.md)
[ -n "$(echo "$notes" | tr -d '[:space:]')" ] || die 'nothing under "## Unreleased" in CHANGELOG.md'
[ "$(mac uname -sm)" = "Darwin x86_64" ] || die "can't reach an Intel Mac at $MAC:$MAC_PORT"

echo "== testing"
mise exec -- cargo test --workspace -q >/dev/null || die "tests failed"

# ------------------------------------------------------- commit and tag
echo "== releasing $tag"
sed -i "s/^version = \".*\"/version = \"$version\"/" Cargo.toml
mise exec -- cargo update --workspace --offline -q
awk -v head="## $version ($(date +%F))" '
    /^## Unreleased/ && !done { print; print ""; print head; done = 1; next }
    { print }' CHANGELOG.md >CHANGELOG.md.new
mv CHANGELOG.md.new CHANGELOG.md
git commit -q -am "Release $version"
git tag -a "$tag" -m "omabubble $version" -m "$notes"
commit=$(git rev-parse --short=7 HEAD)

# Until the publish step, a failure leaves only the local commit and tag.
published=
trap '[ $? = 0 ] || [ -n "$published" ] || echo "release: nothing was published; to undo: git tag -d $tag && git reset --hard HEAD~1" >&2' EXIT

# ---------------------------------------------------------------- build
dist=target/dist/$tag
rm -rf "$dist"
mkdir -p "$dist"

echo "== building omabubble"
mise exec -- cargo build --release -q -p tui
cp target/release/omabubble "$dist/omabubble-linux-amd64"
want="omabubble $version ($commit)"
got=$("$dist/omabubble-linux-amd64" --version)
[ "$got" = "$want" ] || die "omabubble says \"$got\", expected \"$want\""

echo "== building and signing omabubble-bridge on the Mac"
mac "mkdir -p $MAC_DIR"
rsync -a --delete --exclude target --exclude .git -e "ssh -p $MAC_PORT" ./ "$MAC:$MAC_DIR/"
out=target/release/omabubble-bridge-darwin-amd64
mac "set -e
    cd $MAC_DIR
    OMABUBBLE_COMMIT=$commit MISE_QUIET=1 $MAC_MISE exec -- cargo build --release -q -p bridge
    cp target/release/omabubble-bridge $out
    scripts/mac/sign-bridge.sh $out
    scripts/mac/sign-bridge.sh --check $out" || die "building or signing the bridge failed"
want="omabubble-bridge $version ($commit)"
got=$(mac "$MAC_DIR/$out --version")
[ "$got" = "$want" ] || die "omabubble-bridge says \"$got\", expected \"$want\""
scp -q -P "$MAC_PORT" "$MAC:$MAC_DIR/$out" "$dist/"

printf '# omabubble %s\n%s\n' "$version" "$notes" >"$dist/RELEASE-NOTES.md"

# -------------------------------------------------------------- publish
echo "== publishing"
published=1
artifact-push omabubble "$tag" "$dist/omabubble-linux-amd64" "$dist/RELEASE-NOTES.md"
artifact-push omabubble-bridge "$tag" "$dist/omabubble-bridge-darwin-amd64" "$dist/RELEASE-NOTES.md"
git push -q origin main "$tag"

cat <<EOF

Released omabubble $version ($commit).

Install the TUI (to ~/.local/bin/omabubble):
  curl -fsSL $SITE/install.sh | sh -s -- omabubble $tag
Install the bridge, in a terminal (asks for the Mac password):
  ssh -t -p $MAC_PORT $MAC '~/$MAC_DIR/scripts/mac/install-bridge.sh --release $tag'
EOF
