#!/usr/bin/env bash
#
# Packs one edition of a release for one target, from binaries already built.
#
#     packaging/package.sh cli     VERSION TARGET path/to/snob
#     packaging/package.sh desktop VERSION TARGET path/to/snob path/to/snob-desktop
#
# **This is the one definition of what an edition's archive holds**; the
# release workflow runs it and nothing else writes an archive.
#
#   cli      snob-v<version>-<target>           snob
#   desktop  snob-desktop-v<version>-<target>   snob-desktop and snob
#
# both with README.md and LICENSE, under one directory named as the archive.
# The command line's archive keeps the name every released snob updates from
# and Homebrew, Scoop and WinGet install from. The desktop edition's `snob`
# is the very file the command line's archive holds, handed over, never
# built a second time; on Linux that is the static one, in an archive named
# for the app's glibc target.
#
# The archive is written to the directory this is run from, as a .zip for a
# Windows target and a .tar.gz otherwise: a .tar.gz keeps the executable bit
# a .zip would lose on Unix, and Windows opens a .zip with nothing installed.
#
# Two runs on one commit give the same bytes. Both formats embed a
# modification time, so both are given one: SOURCE_DATE_EPOCH, else the
# committer date of the commit checked out.

set -euo pipefail

usage="usage: package.sh cli|desktop VERSION TARGET SNOB [APP]"
edition="${1:?$usage}"
version="${2:?$usage}"
target="${3:?$usage}"
snob="${4:?$usage}"
root="$(cd "$(dirname "$0")/.." && pwd)"

case "$target" in
  *windows*) exe=.exe ;;
  *) exe= ;;
esac

case "$edition" in
  cli)
    name="snob-v$version-$target"
    ;;
  desktop)
    app="${5:?the desktop edition needs the app too: $usage}"
    name="snob-desktop-v$version-$target"
    ;;
  *)
    echo "$usage" >&2
    exit 2
    ;;
esac

epoch="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}"

rm -rf "staging/$name"
mkdir -p "staging/$name"
cp "$root/README.md" "$root/LICENSE" "staging/$name/"
# Under the names an install looks for, whatever the files handed in are
# called.
cp "$snob" "staging/$name/snob$exe"
if [ "$edition" = desktop ]; then
  cp "$app" "staging/$name/snob-desktop$exe"
fi

if [ -n "$exe" ]; then
  # Compress-Archive rather than 7z: it ships with PowerShell, and the
  # Windows on ARM image cannot be assumed to carry 7-Zip.
  #
  # It stores each entry's LastWriteTime, and there is no flag for that, so
  # the times are set on the files first. A zip's DOS timestamp has
  # two-second resolution, so the value is rounded down to an even second
  # here rather than left for the format to round unpredictably.
  even=$((epoch - epoch % 2))
  rm -f "$name.zip"
  (cd staging && powershell -NoProfile -Command \
    "\$stamp = [DateTimeOffset]::FromUnixTimeSeconds($even).UtcDateTime; \
     Get-ChildItem -Recurse '$name' | ForEach-Object { \$_.LastWriteTimeUtc = \$stamp }; \
     Compress-Archive -Path '$name' -DestinationPath '../$name.zip'")
  echo "$name.zip"
else
  # The modes are part of the bytes: said here rather than left to whatever
  # the files arrived with (a binary handed from one job to another loses
  # its executable bit).
  chmod 755 "staging/$name"
  chmod 644 "staging/$name"/*
  chmod 755 "staging/$name/snob"
  if [ "$edition" = desktop ]; then
    chmod 755 "staging/$name/snob-desktop"
  fi
  # --sort=name so the order does not depend on the filesystem, --mtime so
  # it does not depend on when this ran, and --owner/--group so it does not
  # depend on who the runner is. gzip -n because gzip writes its own
  # timestamp into the header as well, which is the one everybody forgets.
  #
  # Those flags are GNU tar's, and macOS ships BSD tar as `tar`, which
  # refuses --sort=name outright. The runner image carries GNU tar as
  # `gtar`, so that name is preferred wherever it exists. The fallback to
  # plain `tar` is for Linux, where it is GNU; a macOS host without gtar
  # still fails loudly on the first flag rather than quietly building an
  # archive whose bytes depend on the filesystem.
  tar="$(command -v gtar || command -v tar)"
  "$tar" --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
    -C staging -cf - "$name" | gzip -n > "$name.tar.gz"
  echo "$name.tar.gz"
fi

rm -rf "staging/$name"
rmdir staging 2>/dev/null || true
