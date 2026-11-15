# The Linux job of .github/workflows/ci.yml, runnable on this machine.
#
# It is the job that differs most from a developer's host: a musl target, a
# real Secret Service keyring behind a session bus, and a glibc-free link.
# Waiting on a remote runner to learn that a change broke one of those costs
# ten minutes a round; this image answers in under a minute once its caches
# are warm. The Windows and macOS jobs are not here, because a container
# cannot run either -- the host is the Windows job.
#
# The image pins the same compiler rust-toolchain.toml pins, so what is
# tested here is what ships. Bump the two together.
FROM rust:1.100.0-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        musl-tools gnome-keyring dbus-x11 libdbus-1-3 file curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# Pinned to the version .github/workflows/ci.yml installs, into /usr/local/bin
# because run.sh mounts the registry volume over /cargo at start.
RUN set -eu; \
    case "$(uname -m)" in \
      x86_64)  sha="84f0f9ba75d0b6fa117bc418d49747ba78e20e4583057f6fa628f1b17c17234d" ;; \
      aarch64) sha="5a7fe62b64a9006e1e47422021df7b1d308cc76c528c9eaf4649a488615ed6fb" ;; \
      *) exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.150/cargo-nextest-0.9.150-$(uname -m)-unknown-linux-musl.tar.gz" \
      -o /tmp/nextest.tar.gz; \
    echo "$sha  /tmp/nextest.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/nextest.tar.gz -C /usr/local/bin cargo-nextest; \
    rm /tmp/nextest.tar.gz
# The musl target of whichever architecture this image is built for: x86_64
# on an Intel host, aarch64 on an ARM64 one. ci.sh picks the same string the
# same way, and `musl-tools` only ever installs the host's own musl-gcc.
RUN rustup target add "$(uname -m)-unknown-linux-musl" \
    && rustup component add rustfmt clippy

ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0 \
    CARGO_HOME=/cargo \
    CARGO_TARGET_DIR=/target
WORKDIR /src
COPY ci.sh /ci.sh
ENTRYPOINT ["bash", "/ci.sh"]
