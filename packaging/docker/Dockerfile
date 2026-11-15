# snob's command line and MCP server in a container: the cli edition, with no
# app (`snob_engine::update::edition`).
#
#     docker build --build-arg VERSION=0.9.0 -t snob packaging/docker
#     docker run --rm -it -v snob-data:/data snob login --paste
#     docker run --rm -i -v snob-data:/data -e SNOB_MCP_KEY snob mcp
#
# **Not published anywhere**: an image is built from a released binary by
# whoever wants one. The binary is the static one of the release, checked
# against its SHA256SUMS before anything else is done with it.
#
# What a container changes, and why:
# - No keyring: sessions are kept with `--no-keyring` (the entrypoint says it
#   for every command), a 0600 file under /data. The MCP key is never kept in
#   a file: hand it in as `SNOB_MCP_KEY`.
# - `snob mcp` serves over standard input and output (`docker run -i`).
#   `--listen` binds loopback only, which is the container's own.
# - Never root: the browser refuses it.
# - No self-update: a container is replaced by a newer image.
# - Every file snob keeps is under the volume at /data.

FROM debian:trixie-slim

# The release to install; required.
ARG VERSION
# Set by `docker build` for the platform being built.
ARG TARGETARCH

# Chromium is the browser snob sends its requests from; the fonts are what
# it renders a page with. Nothing else is needed at run time.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl chromium fonts-liberation \
 && rm -rf /var/lib/apt/lists/*

RUN set -eu; \
    test -n "$VERSION" || { echo "--build-arg VERSION=<version> is required" >&2; exit 1; }; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "snob has no build for $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    name="snob-v$VERSION-$target"; \
    base="https://github.com/dennisgr7/snob/releases/download/v$VERSION"; \
    cd /tmp; \
    curl -fsSLO "$base/$name.tar.gz"; \
    curl -fsSLO "$base/SHA256SUMS"; \
    grep " $name.tar.gz\$" SHA256SUMS | sha256sum -c -; \
    tar -xzf "$name.tar.gz"; \
    install -m 755 "$name/snob" /usr/local/bin/snob; \
    rm -rf /tmp/*; \
    snob --version

RUN useradd --create-home --uid 10001 snob \
 && mkdir /data && chown snob:snob /data

USER snob
ENV XDG_CONFIG_HOME=/data/config \
    XDG_DATA_HOME=/data/share \
    SNOB_NO_UPDATE_CHECK=1
VOLUME /data

ENTRYPOINT ["snob", "--no-keyring"]
CMD ["--help"]
