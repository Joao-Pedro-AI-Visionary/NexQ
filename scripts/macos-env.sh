#!/usr/bin/env bash
# Source before building NexQ on macOS:  source scripts/macos-env.sh
#
# src-tauri/.cargo/config.toml carries Windows defaults for LIBCLANG_PATH and
# CMAKE_GENERATOR. Cargo's [env] never overrides variables that are already
# set, so exporting macOS values here wins.

# libclang (bindgen for whisper-rs): Xcode, the Command Line Tools, or Homebrew LLVM.
if [[ -z "${LIBCLANG_PATH:-}" || ! -f "${LIBCLANG_PATH}/libclang.dylib" ]]; then
  _dev="$(xcode-select -p 2>/dev/null || true)"
  for _dir in \
    "${_dev}/Toolchains/XcodeDefault.xctoolchain/usr/lib" \
    "${_dev}/usr/lib" \
    "/Library/Developer/CommandLineTools/usr/lib" \
    "$(brew --prefix llvm 2>/dev/null || true)/lib"; do
    if [[ -f "${_dir}/libclang.dylib" ]]; then
      export LIBCLANG_PATH="${_dir}"
      break
    fi
  done
  unset _dev _dir
fi
[[ -n "${LIBCLANG_PATH:-}" ]] || echo "⚠ libclang.dylib not found — install the Xcode Command Line Tools: xcode-select --install" >&2

# Ninja when installed (faster), else CMake's default Makefiles.
if command -v ninja >/dev/null 2>&1; then
  export CMAKE_GENERATOR="Ninja"
else
  export CMAKE_GENERATOR="Unix Makefiles"
fi
