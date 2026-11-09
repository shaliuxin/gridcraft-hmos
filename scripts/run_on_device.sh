#!/usr/bin/env bash
# 本地开发循环：编译 Rust 适配层（arm64）→ hvigor 打包 → 安装到已连接设备 → 启动。
# 仅用于本机调试；正式构建请走 build_native.sh（双架构）+ DevEco 签名。
set -uo pipefail

DEVECO="${DEVECO_HOME:-/c/Program Files/Huawei/DevEco Studio}"
SDKLLVM="$DEVECO/sdk/default/openharmony/native/llvm/bin"
HDC="$DEVECO/sdk/default/openharmony/toolchains/hdc.exe"
SDK_USER="${OHOS_SDK_USER:-/c/Users/mingh/AppData/Local/Huawei/Sdk}"
TARGET="${OHOS_TARGET:-aarch64-unknown-linux-ohos}"
BUNDLE="${GC_BUNDLE:-io.github.shaliuxin.gridcraft.hmos}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

DEVICE="${1:-}"
if [ -z "$DEVICE" ]; then
  DEVICE="$("$HDC" list targets 2>/dev/null | tr -d '\r' | head -1)"
fi
[ -n "$DEVICE" ] || { echo "no device connected"; exit 1; }
echo "device=$DEVICE target=$TARGET bundle=$BUNDLE"

export CARGO_NET_OFFLINE=true
export DEVECO_SDK_HOME="$DEVECO/sdk"
export NODE_HOME="$DEVECO/tools/node"
export JAVA_HOME="$DEVECO/jbr"
export PATH="$DEVECO/jbr/bin:$DEVECO/tools/node:$DEVECO/tools/hvigor/bin:$PATH"

echo "--- [1/4] cargo build ---"
cd "$ROOT/native/rust" || exit 1
export CC_aarch64_unknown_linux_ohos="$SDKLLVM/clang.exe"
export AR_aarch64_unknown_linux_ohos="$SDKLLVM/llvm-ar.exe"
export CC_x86_64_unknown_linux_ohos="$SDKLLVM/clang.exe"
export AR_x86_64_unknown_linux_ohos="$SDKLLVM/llvm-ar.exe"
# 万一有传递依赖的 C 语言 cc-crate（如 wgpu-hal 的 C 绑定），交叉编译走 CXX。
# 鸿蒙 clang++ 的真身是 clang++.exe；包装脚本 *-clang++ 只是给它加上
#  -target aarch64-linux-ohos --sysroot=<native>/sysroot -D__MUSL__；这些标志放进 CXXFLAGS。
# sysroot 路径含空格（Program Files），cc 会按空格拆 --sysroot=… 参数。
# 用无空格 junction C:\ohossysroot → <native>/sysroot（PowerShell 建的 mklink /J）。
# 关键：clang++.exe 是原生 Windows 可执行文件，不认 MSYS 的 /c/... 路径，
# 必须用原生 Windows 反斜杠路径 C:\ohossysroot。
OHOS_SYSROOT="${OHOS_SYSROOT:-C:\\ohossysroot}"
export CXX_aarch64_unknown_linux_ohos="$SDKLLVM/clang++.exe"
export CXX_x86_64_unknown_linux_ohos="$SDKLLVM/clang++.exe"
export CXXFLAGS_aarch64_unknown_linux_ohos="-target aarch64-linux-ohos --sysroot=$OHOS_SYSROOT -D__MUSL__"
export CXXFLAGS_x86_64_unknown_linux_ohos="-target x86_64-linux-ohos --sysroot=$OHOS_SYSROOT -D__MUSL__"
cargo build --release --target "$TARGET" 2>&1 | tail -8
[ "${PIPESTATUS[0]}" -eq 0 ] || { echo "cargo build failed"; exit 1; }

echo "--- [2/4] assembleHap ---"
cd "$ROOT" || exit 1
# 仓库里的 build-profile.json5 是占位符（不含本机签名路径 / 口令 hex）；
# 若存在本地真实配置 signing/build-profile.local.json5（已 gitignore），打包前自动还原。
if [ -f "$ROOT/signing/build-profile.local.json5" ]; then
  cp -f "$ROOT/signing/build-profile.local.json5" "$ROOT/build-profile.json5"
  echo "restored local signing config from signing/build-profile.local.json5"
fi
node "$DEVECO/tools/hvigor/bin/hvigorw.js" assembleHap --mode module \
  -p product=default -p buildMode=debug --no-daemon 2>&1 | tail -8
[ "${PIPESTATUS[0]}" -eq 0 ] || { echo "assembleHap failed"; exit 1; }

echo "--- [3/4] install ---"
cd "$ROOT/entry/build/default/outputs/default" || exit 1
"$HDC" -t "$DEVICE" uninstall "$BUNDLE" 2>&1 | tr -d '\r' | tail -1
"$HDC" -t "$DEVICE" install entry-default-signed.hap 2>&1 | tr -d '\r' | tail -2

echo "--- [4/4] launch ---"
"$HDC" -t "$DEVICE" shell hilog -r 2>&1 | tr -d '\r' | tail -1
"$HDC" -t "$DEVICE" shell aa start -b "$BUNDLE" -a EntryAbility 2>&1 | tr -d '\r' | tail -2
echo "done."
