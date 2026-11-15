#!/bin/sh
#
# Installs snob on Linux or macOS.
#
#     curl -fsSL https://raw.githubusercontent.com/dennisgr7/snob/main/packaging/install.sh | sh
#
# Reads three variables, all optional:
#
#     SNOB_VERSION      a version to install instead of the latest
#     SNOB_INSTALL_DIR  where to put the binaries; default ~/.local/bin
#     SNOB_EDITION      desktop (the app, with the command line and the MCP
#                       server beside it) or cli (those two alone, for a
#                       server or a container)
#
# Without SNOB_EDITION the desktop edition is installed where the release
# lists it for this machine, and the command line alone otherwise, said so.
# The app's two binaries go to the same directory: the app starts the
# `snob` beside it, and that is also how an update knows the edition.
#
# POSIX sh rather than bash: this is the one file that has to run before the
# user has installed anything, so it cannot assume a shell that some minimal
# containers do not ship.

set -eu

REPO="dennisgr7/snob"
INSTALL_DIR="${SNOB_INSTALL_DIR:-$HOME/.local/bin}"

die() {
  echo "error: $*" >&2
  exit 1
}

# Which archive of the command line this machine needs.
target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os/$arch" in
    Linux/x86_64) echo "x86_64-unknown-linux-musl" ;;
    Linux/aarch64 | Linux/arm64) echo "aarch64-unknown-linux-musl" ;;
    Darwin/arm64) echo "aarch64-apple-darwin" ;;
    # A shell running under Rosetta reports x86_64 on an Apple Silicon Mac,
    # which is not an Intel Mac: the kernel says which it really is.
    Darwin/x86_64)
      if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
        echo "aarch64-apple-darwin"
        return
      fi
      # Named rather than lumped in with the unknown: an Intel Mac is a
      # machine somebody actually has, and "unsupported platform" would not
      # tell them that building from source is right there.
      die "snob has no build for Intel Macs. Build it from source instead:
    cargo install --locked --git https://github.com/$REPO snob-cli"
      ;;
    *) die "no build for $os on $arch" ;;
  esac
}

# The desktop edition's target here, or nothing: glibc on Linux, which the
# app's toolkit needs (the `snob` inside is the static one all the same),
# and none on macOS yet.
desktop_target() {
  case "$1" in
    x86_64-unknown-linux-musl) echo "x86_64-unknown-linux-gnu" ;;
    aarch64-unknown-linux-musl) echo "aarch64-unknown-linux-gnu" ;;
  esac
}

# Whether SHA256SUMS lists a file.
listed() {
  awk -v f="$1" '$2 == f || $2 == "*"f {found = 1} END {exit !found}' "$2"
}

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    die "neither curl nor wget is installed"
  fi
}

# The checksum is not optional. This script downloads an executable over the
# network and puts it on the user's PATH; verifying it is the least it can do.
verify() {
  archive="$1" sums="$2"
  expected=$(awk -v f="$archive" '$2 == f || $2 == "*"f {print $1}' "$sums")
  [ -n "$expected" ] || die "$archive is not listed in SHA256SUMS"

  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$archive" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$archive" | awk '{print $1}')
  else
    die "no sha256sum or shasum to check the download with"
  fi

  [ "$expected" = "$actual" ] || die "checksum mismatch for $archive"
}

# The checksum says the file is the one the release page lists. It does not
# say who built it: SHA256SUMS is fetched from the same page as the archive,
# so whoever can replace one can replace both. The release workflow signs
# build provenance through Sigstore for exactly that question, and this is
# where somebody finally asks it. `gh` is the only client that reads those
# attestations, so without it the step is skipped and said so -- a check that
# silently did not run is the one outcome worse than no check.
#
# The provenance keeps the name the repository had when it was signed, which
# was `snob-cli` up to 0.9.0.
signed_in() {
  old_ifs=$IFS
  IFS=.
  # shellcheck disable=SC2086
  set -- $1
  IFS=$old_ifs
  if [ "${1:-0}" -eq 0 ] && [ "${2:-0}" -le 9 ]; then
    echo "dennisgr7/snob-cli"
  else
    echo "$REPO"
  fi
}

attest() {
  archive="$1"
  if ! command -v gh >/dev/null 2>&1; then
    echo "note: gh is not installed, so the build provenance was not verified"
    echo "      (the checksum was). Install GitHub CLI to have it checked."
    return 0
  fi
  # A check that could not run is not a check that failed. `gh attestation`
  # arrived in 2.49 -- Debian 13 ships 2.46 -- and it needs a login; either
  # way, dying with "does not carry a valid build provenance" accused the
  # release of tampering for something about this machine.
  if ! gh attestation --help >/dev/null 2>&1 || ! gh auth status >/dev/null 2>&1; then
    echo "note: this gh cannot verify attestations (it is older than 2.49, or not"
    echo "      logged in), so the build provenance was not verified (the checksum was)."
    return 0
  fi
  signer=$(signed_in "$version")
  gh attestation verify "$archive" --repo "$signer" \
    --signer-workflow "$signer/.github/workflows/release.yml" >/dev/null \
    || die "$archive does not carry a valid build provenance from $signer's release workflow"
  echo "Build provenance verified"
}

main() {
  t=$(target)

  # A leading v is how the tags are spelled and how people type a version;
  # the archive names carry one of their own, so it is taken off here.
  version="${SNOB_VERSION:-}"
  version="${version#v}"
  if [ -z "$version" ]; then
    tmp_tag=$(mktemp)
    download "https://api.github.com/repos/$REPO/releases/latest" "$tmp_tag"
    version=$(sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' "$tmp_tag" | head -1)
    rm -f "$tmp_tag"
    [ -n "$version" ] || die "could not work out the latest version"
  fi

  base="https://github.com/$REPO/releases/download/v$version"

  work=$(mktemp -d)
  # Whatever happens next, the download does not outlive this script.
  trap 'rm -rf "$work"' EXIT INT TERM

  download "$base/SHA256SUMS" "$work/SHA256SUMS"

  # Which edition, from what the release lists for this machine.
  edition="${SNOB_EDITION:-}"
  dt=$(desktop_target "$t")
  desktop="snob-desktop-v$version-$dt"
  case "$edition" in
    desktop)
      [ -n "$dt" ] || die "snob has no desktop app for this machine yet; SNOB_EDITION=cli installs the command line"
      listed "$desktop.tar.gz" "$work/SHA256SUMS" ||
        die "snob $version has no desktop app for this machine; SNOB_EDITION=cli installs the command line"
      ;;
    cli) ;;
    "")
      if [ -n "$dt" ] && listed "$desktop.tar.gz" "$work/SHA256SUMS"; then
        edition=desktop
      else
        edition=cli
        echo "snob $version has no desktop app for this machine: installing the command line and the MCP server"
      fi
      ;;
    *) die "SNOB_EDITION is desktop or cli, not $edition" ;;
  esac
  if [ "$edition" = desktop ]; then
    name="$desktop"
    echo "Downloading snob $version, the desktop app, for $dt"
  else
    name="snob-v$version-$t"
    echo "Downloading snob $version for $t"
  fi
  download "$base/$name.tar.gz" "$work/$name.tar.gz"

  (cd "$work" && verify "$name.tar.gz" SHA256SUMS)
  attest "$work/$name.tar.gz"
  tar -xzf "$work/$name.tar.gz" -C "$work"

  mkdir -p "$INSTALL_DIR"
  # The app first and the command line last, as an update replaces them.
  if [ "$edition" = desktop ]; then
    install -m 755 "$work/$name/snob-desktop" "$INSTALL_DIR/snob-desktop"
  fi
  install -m 755 "$work/$name/snob" "$INSTALL_DIR/snob"

  echo "Installed $("$INSTALL_DIR/snob" --version) to $INSTALL_DIR"

  # Which profile file to write is a question only the user's shell can
  # answer, so this prints the line rather than guessing at one. What it must
  # not do is then tell them to run a command that will not resolve.
  on_path=yes
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      on_path=no
      echo
      echo "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
      echo "    export PATH=\"\$PATH:$INSTALL_DIR\""
      ;;
  esac

  # Both lines, not just the first. The purge reminder is the one command here
  # it matters most that somebody can actually type, because skipping it
  # leaves a live session cookie on a machine whose owner has just uninstalled
  # the tool.
  if [ "$on_path" = yes ]; then
    snob=snob
  else
    snob="$INSTALL_DIR/snob"
  fi

  echo
  if [ "$edition" = desktop ]; then
    echo "Open the app with: $INSTALL_DIR/snob-desktop"
  fi
  echo "Start with: $snob login"
  echo "Before uninstalling, run \"$snob purge\": the session and the database"
  echo "live outside this directory and deleting the binary will not reach them."
}

main
