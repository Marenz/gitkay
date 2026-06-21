#!/usr/bin/env bash
# Install gitkay into ~/.cargo/bin (Cargo's default binary directory).
#
# Usage: ./install.sh [extra cargo args]
#
# Requires the build dependencies listed in the README
# (libgtk-4, libgraphene, libssl, pkg-config, cmake).

cd "$(dirname "$0")"

exec cargo install --path . --locked --force "$@"
