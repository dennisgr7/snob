# The desktop app's Linux job, runnable on this machine: the image of the
# command line's job (tools/ci/Dockerfile) with what GPUI needs to build and
# to draw without a screen.
#
# **It starts from `snob-ci`, so the compiler is pinned in one place**:
# build that image first (`docker build -t snob-ci tools/ci`).
ARG BASE=snob-ci
FROM ${BASE}

# What GPUI links on Linux (xkbcommon, Wayland, XCB, fontconfig), and a
# display and a GPU that exist nowhere: Xvfb, and Mesa's Vulkan on the CPU
# (lavapipe), which GPUI's renderer accepts as its last choice.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
        libxcb1-dev libx11-xcb-dev libfontconfig1-dev libfreetype6-dev \
        libvulkan1 mesa-vulkan-drivers xvfb xauth \
    && rm -rf /var/lib/apt/lists/*

# A target directory of its own: the app is glibc, the command line's job
# is musl, and one must not evict the other's cache.
ENV CARGO_TARGET_DIR=/target-desktop
WORKDIR /src/apps/desktop
ENTRYPOINT []
