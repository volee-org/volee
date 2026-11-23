#!/bin/sh
# Builds the release artifacts for THIS machine with dist, then runs the real
# dist installer against them, served from 127.0.0.1. Nothing is downloaded
# from a GitHub release and nothing is published: this is how the installer is
# tested before a tag exists.
#
#   scripts/install-local.sh [PREFIX]
#
# PREFIX is the directory the `volee` binary goes in (default
# ~/.local/volee-v2/bin). It never touches PATH or a shell profile.
set -eu

prefix="${1:-$HOME/.local/volee-v2/bin}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

target="$(rustc -vV | sed -n 's/^host: //p')"
echo "target: $target"

# The daemon embeds ui/dist; a release build refuses to go on without it.
if [ ! -f ui/dist/index.html ]; then
    (cd ui && npm ci && npm run build)
fi
export VOLEE_REQUIRE_UI=1
dist build --artifacts=local --target "$target"
dist build --artifacts=global

work="$(mktemp -d)"
server=""
cleanup() {
    [ -n "$server" ] && kill "$server" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT INT TERM

cp target/distrib/volee-cli-installer.sh "$work/"
cp target/distrib/volee-cli-"$target".tar.xz "$work/"
cp target/distrib/volee-cli-"$target".tar.xz.sha256 "$work/"

# The installer cannot verify a checksum it was not generated with (CI builds
# it after the archives, so the published one can): check it here.
(cd "$work" && shasum -a 256 -c "volee-cli-$target.tar.xz.sha256")

port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work" >/dev/null 2>&1 &
server=$!
sleep 1

# VOLEE_CLI_DOWNLOAD_URL replaces the GitHub release URL; INSTALL_DIR puts the
# binary in PREFIX instead of ~/.local/bin; NO_MODIFY_PATH leaves the shell
# profile alone.
VOLEE_CLI_DOWNLOAD_URL="http://127.0.0.1:$port" \
    VOLEE_CLI_INSTALL_DIR="$prefix" \
    VOLEE_CLI_NO_MODIFY_PATH=1 \
    sh "$work/volee-cli-installer.sh"

"$prefix/volee" --version

# `volee2`: the same binary with directories of its own. By default volee v2 and
# volee 0.x share ~/.local/state/volee (database, lock, socket), which makes v2 talk
# to a running 0.x daemon, even stop it. This wrapper keeps them apart.
cat >"$prefix/volee2" <<'WRAPPER'
#!/bin/sh
# volee v2, kept apart from volee 0.x: its own configuration, state and data.
# VOLEE_V2_HOME moves all three (default ~/.local/volee-v2/home).
home="${VOLEE_V2_HOME:-$HOME/.local/volee-v2/home}"
VOLEE_CONFIG_DIR="${VOLEE_CONFIG_DIR:-$home/config}"
VOLEE_STATE_DIR="${VOLEE_STATE_DIR:-$home/state}"
VOLEE_DATA_DIR="${VOLEE_DATA_DIR:-$home/data}"
VOLEE_SOCKET="${VOLEE_SOCKET:-$VOLEE_STATE_DIR/volee.sock}"
export VOLEE_CONFIG_DIR VOLEE_STATE_DIR VOLEE_DATA_DIR VOLEE_SOCKET
exec "$(dirname "$0")/volee" "$@"
WRAPPER
chmod +x "$prefix/volee2"
echo "installed $prefix/volee2 (volee v2 with its own directories)"
