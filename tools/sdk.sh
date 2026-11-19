#!/bin/sh
# Runs a command inside the GNOME 51 SDK with the Rust extension and the
# locally built VTE (target-sdk/deps/prefix) and the bundled fonts, e.g. `tools/sdk.sh cargo build -p anvil`.
cd "$(dirname "$0")/.."
ROOT="$PWD"
exec flatpak run --share=network --filesystem="$ROOT" --filesystem=home --command=bash org.gnome.Sdk//51 -c "
export PATH=\$PATH:/usr/lib/sdk/rust-stable/bin CARGO_TARGET_DIR=$ROOT/target-sdk
export PKG_CONFIG_PATH=$ROOT/target-sdk/deps/prefix/lib64/pkgconfig:\$PKG_CONFIG_PATH
export LD_LIBRARY_PATH=$ROOT/target-sdk/deps/prefix/lib64:\$LD_LIBRARY_PATH
mkdir -p $ROOT/target-sdk/fonts
printf '<?xml version=\"1.0\"?><fontconfig><include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include><dir>$ROOT/insights-mono/fonts</dir><cachedir>$ROOT/target-sdk/fonts</cachedir></fontconfig>' > $ROOT/target-sdk/fonts/fonts.conf
export FONTCONFIG_FILE=$ROOT/target-sdk/fonts/fonts.conf
cd $ROOT && $*"
