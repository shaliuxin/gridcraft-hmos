#!/usr/bin/env bash
# Build the HarmonyOS native libraries (every ABI in GRIDCRAFT_NATIVE_TARGETS) before hvigor/CMake.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -f "$ROOT/scripts/dev.local.env" ]]; then source "$ROOT/scripts/dev.local.env"; fi
# gridcraft 的界面 i18n 是内嵌在 crates/ui-egui/src/i18n/*.rs（zh.rs 为真实简体中文），
# 无 CRAFT_I18N_DIR / 外部 tsv；语言由 Language::system() 读 LC_* 决定，C++ 侧 install_locale 设之。
# 故本脚本不再设置 CRAFT_I18N_DIR。
# Resolve the OHOS native SDK across Windows / macOS / Linux DevEco layouts.
# GRIDCRAFT_NATIVE_SDK (or dev.local.env) wins; otherwise probe known locations, including the
# Windows default install path and the space-free junction C:/ohosnative used by .cargo/config.toml.
if [[ -n "${GRIDCRAFT_NATIVE_SDK:-}" ]]; then
  SDK="$GRIDCRAFT_NATIVE_SDK"
else
  SDK=""
  for candidate in \
    "${DEVECO_SDK_HOME:-}/default/openharmony/native" \
    "${HOS_SDK_HOME:-}/default/openharmony/native" \
    "/c/Program Files/Huawei/DevEco Studio/sdk/default/openharmony/native" \
    "/c/ohosnative" \
    "/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native" \
    "${DEVECO_SDK_HOME:-}/openharmony/native" \
    "${DEVECO_SDK_HOME:-}/native" \
    "${DEVECO_SDK_HOME:-}"; do
    if [[ -d "$candidate/llvm" && -d "$candidate/sysroot" ]]; then SDK="$candidate"; break; fi
  done
  [[ -n "$SDK" ]] || SDK="/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"
fi
{ [[ -x "$SDK/llvm/bin/clang" || -f "$SDK/llvm/bin/clang.exe" ]]; } || { echo "HarmonyOS native SDK not found: $SDK" >&2; exit 1; }
# One list drives the build: CMake maps the same ABIs (entry/build-profile.json5 abiFilters), and
# verify_native.py checks the packaged set. Both ABIs ship by default so one HAP runs on ARM
# devices and on the x86_64 phone/PC emulator images.
TARGETS="${GRIDCRAFT_NATIVE_TARGETS:-aarch64-unknown-linux-ohos x86_64-unknown-linux-ohos}"
PROFILE="${GRIDCRAFT_RUST_PROFILE:-release}"
# Windows 上 Defender/文件锁偶发让 rustc 挂死（进程 CPU 长时间 ~0）。把并行度限到 4
# 并关掉增量编译可稳定规避（两次实测有效）；想更快可 export GRIDCRAFT_CARGO_JOBS=<n>。
JOBS="${GRIDCRAFT_CARGO_JOBS:-${CARGO_BUILD_JOBS:-4}}"
case "$PROFILE" in
  release) ARGS+=(--release) ;;
  debug)   ARGS+=(--profile dev) ;;
  *) echo 'Profile must be release or debug' >&2; exit 2 ;;
esac
for target in $TARGETS; do
  case "$target" in
    aarch64-unknown-linux-ohos|x86_64-unknown-linux-ohos|armv7-unknown-linux-ohos) ;;
    *) echo "Unsupported target: $target" >&2; exit 2;;
  esac
  [[ -d "$(rustc --print sysroot)/lib/rustlib/$target" ]] || { echo "Install target with: rustup target add $target" >&2; exit 1; }
  suffix="${target//-/_}"
  upper="$(printf '%s' "$suffix" | tr '[:lower:]' '[:upper:]')"
  # Windows: the `*-unknown-linux-ohos-clang` tools are POSIX sh wrappers that Rust cannot spawn
  # (os error 193). Drive both the cc-crate C probe and the final link through clang.exe directly,
  # and carry the OHOS target flags via CFLAGS (cc-rs already injects its own --target too).
  # macOS/Linux keep the wrapper (it bakes target+sysroot).
  #
  # Two traps in CFLAGS on Windows:
  #   1) cc-rs WORD-SPLITS CFLAGS, so a spaced --sysroot ("C:\Program Files\...") breaks.
  #   2) cc-rs (a Rust process) passes CFLAGS VERBATIM — unlike bash/MSYS it does NOT translate
  #      `/c/...` to `C:/...`, so the sysroot must already be a Windows-form path.
  # Fix for both: a space-free directory junction C:\ohosnative, referenced as C:/ohosnative
  # (Windows form, no spaces) in both CC and CFLAGS.
  cflags=""
  if [[ -x "$SDK/llvm/bin/clang.exe" ]]; then
    base_win="C:/ohosnative"
    if [[ ! -d "/c/ohosnative/llvm" ]]; then
      win_target="$(cygpath -w "$SDK" 2>/dev/null || printf '%s' "$SDK")"
      powershell -NoProfile -Command "New-Item -ItemType Junction -Path 'C:\\ohosnative' -Target '$win_target' | Out-Null" \
        || { echo "Failed to create junction C:\\ohosnative -> $SDK" >&2; exit 1; }
    fi
    compiler="$base_win/llvm/bin/clang.exe"
    ar="$base_win/llvm/bin/llvm-ar.exe"
    case "$target" in
      aarch64-unknown-linux-ohos) clang_target="aarch64-linux-ohos";;
      x86_64-unknown-linux-ohos)  clang_target="x86_64-linux-ohos";;
      armv7-unknown-linux-ohos)   clang_target="armv7-linux-ohos";;
    esac
    cflags="--target=$clang_target --sysroot=$base_win/sysroot -D__MUSL__"
  else
    compiler="$SDK/llvm/bin/$target-clang"
    ar="$SDK/llvm/bin/llvm-ar"
  fi
  if [[ -n "$cflags" ]]; then
    env "CC_${suffix}=$compiler" "CFLAGS_${suffix}=$cflags" "CFLAGS=$cflags" \
        "AR_${suffix}=$ar" \
        "CARGO_TARGET_${upper}_LINKER=$compiler" \
        "CARGO_BUILD_JOBS=$JOBS" "CARGO_INCREMENTAL=0" \
        cargo build --locked --manifest-path "$ROOT/native/rust/Cargo.toml" --target "$target" "${ARGS[@]}"
  else
    env "CC_${suffix}=$compiler" "AR_${suffix}=$ar" \
        "CARGO_TARGET_${upper}_LINKER=$compiler" \
        "CARGO_BUILD_JOBS=$JOBS" "CARGO_INCREMENTAL=0" \
        cargo build --locked --manifest-path "$ROOT/native/rust/Cargo.toml" --target "$target" "${ARGS[@]}"
  fi
done
