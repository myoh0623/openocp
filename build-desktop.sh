#!/usr/bin/env bash
set -Eeuo pipefail

project_dir="$(cd "$(dirname "$0")" && pwd)"
cd "$project_dir"

if [[ -x "$project_dir/.tools/cargo/bin/cargo" ]]; then
  export RUSTUP_HOME="$project_dir/.tools/rustup"
  export CARGO_HOME="$project_dir/.tools/cargo"
  export PATH="$project_dir/.tools/cargo/bin:$PATH"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust가 없습니다. https://rustup.rs 에서 Rust stable을 설치해 주세요." >&2
  exit 1
fi

if ! command -v npm >/dev/null 2>&1; then
  echo "Node.js와 npm이 필요합니다." >&2
  exit 1
fi

if [[ -f package-lock.json ]]; then
  npm ci
else
  npm install
fi

npm run tauri build -- "$@"
