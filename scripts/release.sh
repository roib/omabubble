#!/bin/sh
# Release omabubble and omabubble-bridge together as one version.
#
#   scripts/release.sh 0.2.0
#
# 1. Checks: clean main, in step with GitHub, version not tagged or released,
#    notes under "## Unreleased" in CHANGELOG.md, tests pass.
# 2. Sets the version in Cargo.toml, dates the changelog section, commits
#    "Release 0.2.0" and tags v0.2.0.
# 3. Builds omabubble here, and omabubble-bridge on your Mac, signed there
#    with the Mac's signing identity so Full Disk Access survives the update.
# 4. Pushes the commit and tag, and creates a GitHub release with the Linux
#    build of omabubble and its checksum. The bridge isn't attached: it's
#    signed for your Mac only. Install it there with
#    `install-bridge.sh --release`.
# 5. Runs ./local-release.sh <tag> <dist dir> if you have one (not committed),
#    e.g. to copy the builds somewhere of your own.
#
# A tag that's been released isn't reused: fix a bad release with a new patch
# version.
#
# Settings come from the environment or from local.env in the repo root (not
# committed; see local.env.example): OMABUBBLE_MAC and OMABUBBLE_MAC_PORT, the
# Mac to build the bridge on, reached with your normal SSH key, not the
# bridge-only one.
set -eu
cd "$(dirname "$0")/.."
[ ! -f local.env ] || . ./local.env

MAC=${OMABUBBLE_MAC:-you@your-mac}
MAC_PORT=${OMABUBBLE_MAC_PORT:-22}
# The Mac's copy of the source, relative to its home: a build mirror that each
# release overwrites (rsync --delete), so it lives with other caches.
MAC_DIR=${OMABUBBLE_MAC_DIR:-Library/Caches/omabubble/release}

die() {
    echo "release: $*" >&2
    exit 1
}
mac() { ssh -p "$MAC_PORT" -o BatchMode=yes "$MAC" "$@"; }
arch() {
    case $1 in
        x86_64 | amd64) echo amd64 ;;
        arm64 | aarch64) echo arm64 ;;
        *) echo "$1" ;;
    esac
}

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
gh auth status >/dev/null 2>&1 || die "gh isn't signed in to GitHub (gh auth login)"
! gh release view "$tag" >/dev/null 2>&1 || die "GitHub already has a release $tag"
notes=$(awk '/^## Unreleased/ {f = 1; next} /^## / {f = 0} f' CHANGELOG.md)
[ -n "$(echo "$notes" | tr -d '[:space:]')" ] || die 'nothing under "## Unreleased" in CHANGELOG.md'
[ "$(mac uname -s)" = Darwin ] || die "can't reach a Mac at $MAC:$MAC_PORT"
mac_arch=$(arch "$(mac uname -m)")
linux_arch=$(arch "$(uname -m)")

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

# Until the push, a failure leaves only the local commit and tag.
pushed=
trap '[ $? = 0 ] || [ -n "$pushed" ] || echo "release: nothing was pushed; to undo: git tag -d $tag && git reset --hard HEAD~1" >&2' EXIT

# ---------------------------------------------------------------- build
dist=target/dist/$tag
rm -rf "$dist"
mkdir -p "$dist"

echo "== building omabubble"
mise exec -- cargo build --release -q -p tui
tui=omabubble-linux-$linux_arch
cp target/release/omabubble "$dist/$tui"
want="omabubble $version ($commit)"
got=$("$dist/$tui" --version)
[ "$got" = "$want" ] || die "omabubble says \"$got\", expected \"$want\""

echo "== building and signing omabubble-bridge on the Mac"
mac "mkdir -p $MAC_DIR"
rsync -a --delete --exclude target --exclude .git --exclude local-release.sh -e "ssh -p $MAC_PORT" ./ "$MAC:$MAC_DIR/"
out=target/release/omabubble-bridge-release
mac "set -e
    cd $MAC_DIR
    OMABUBBLE_COMMIT=$commit scripts/mac/build-bridge.sh $out
    scripts/mac/sign-bridge.sh --check $out" || die "building or signing the bridge failed"
want="omabubble-bridge $version ($commit)"
got=$(mac "$MAC_DIR/$out --version")
[ "$got" = "$want" ] || die "omabubble-bridge says \"$got\", expected \"$want\""
scp -q -P "$MAC_PORT" "$MAC:$MAC_DIR/$out" "$dist/omabubble-bridge-darwin-$mac_arch"

printf '# omabubble %s\n%s\n' "$version" "$notes" >"$dist/RELEASE-NOTES.md"
(cd "$dist" && sha256sum "$tui" >"$tui.sha256")

# -------------------------------------------------------------- publish
echo "== publishing"
pushed=1
git push -q origin main "$tag"
gh release create "$tag" --verify-tag --title "omabubble $version" \
    --notes-file "$dist/RELEASE-NOTES.md" "$dist/$tui" "$dist/$tui.sha256" >/dev/null
if [ -x ./local-release.sh ]; then
    echo "== running local-release.sh"
    ./local-release.sh "$tag" "$dist" || echo "release: local-release.sh failed; the GitHub release is done" >&2
fi

repo=$(gh repo view --json url -q .url)
cat <<EOF

Released omabubble $version ($commit): $repo/releases/tag/$tag

Install the bridge on the Mac, in a terminal (asks for the Mac password):
  ssh -t -p $MAC_PORT $MAC '~/$MAC_DIR/scripts/mac/install-bridge.sh --release'
EOF
