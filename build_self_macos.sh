#!/usr/bin/env bash
# 一键打包 RustDeskSelf（自用定制版）
# 用法: ./build_self_macos.sh
set -euo pipefail

cd "$(dirname "$0")"

APP_NAME="RustDeskSelf"
RELEASE_DIR="flutter/build/macos/Build/Products/Release"

# Flutter SDK（默认外置盘，可用 FLUTTER_BIN 覆盖）
FLUTTER_BIN="${FLUTTER_BIN:-/Volumes/External/flutter/bin}"
if [ ! -x "$FLUTTER_BIN/flutter" ]; then
  echo "错误: 找不到 flutter ($FLUTTER_BIN)，请设置 FLUTTER_BIN"
  exit 1
fi
export PATH="$FLUTTER_BIN:/opt/homebrew/bin:$PATH"

# Xcode（xcode-select 仍指向 CommandLineTools，用 DEVELOPER_DIR 绕过）
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
if ! xcodebuild -version >/dev/null 2>&1; then
  echo "错误: Xcode 不可用，请安装并确认 DEVELOPER_DIR"
  exit 1
fi

echo "==> [1/4] cargo build --release --features flutter"
cargo build --release --features flutter

# Rust FFI 变化时重新生成桥接文件（Dart + C header）
if [ src/flutter_ffi.rs -nt src/bridge_generated.rs ] || \
   [ src/flutter_ffi.rs -nt flutter/lib/generated_bridge.dart ]; then
  echo "==> [2/4] 重新生成 flutter_rust_bridge 绑定"
  export SDKROOT="$(xcrun --show-sdk-path)"
  flutter_rust_bridge_codegen --rust-input src/flutter_ffi.rs \
    --dart-output flutter/lib/generated_bridge.dart \
    --c-output flutter/macos/Runner/bridge_generated.h
else
  echo "==> [2/4] 桥接文件未过期，跳过 codegen"
fi

echo "==> [3/4] flutter build macos --release"
cd flutter
flutter pub get >/dev/null
flutter build macos --release
cd ..

# Xcode 27 下框架签名 Team ID 不一致会导致 dyld 拒绝加载，统一 ad-hoc 重签
echo "==> [4/4] codesign 统一重签"
codesign --force --deep --sign - "$RELEASE_DIR/$APP_NAME.app"

echo "完成: $RELEASE_DIR/$APP_NAME.app"
