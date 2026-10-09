#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plugin_dir="${root_dir}/plugins/calibre/send_to_kobbler"
output_zip="${root_dir}/plugins/send_to_kobbler.zip"

if [[ ! -d "${plugin_dir}" ]]; then
  echo "Plugin directory not found: ${plugin_dir}" >&2
  exit 1
fi

rm -f "${output_zip}"

(
  cd "${plugin_dir}"
  zip -r "${output_zip}" . \
    -x "*.pyc" "__pycache__/*" "*.log" "send_to_kobbler.zip" \
    -x ".ruff_cache/*" "pyproject.toml"
)

echo "Wrote ${output_zip}"
