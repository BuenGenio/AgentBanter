#!/bin/sh
# Run from an extracted release archive. Installs only into the chosen prefix.
set -eu
prefix="${HOME}/.local"
desktop=yes
while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix) [ "$#" -ge 2 ] || { echo 'Missing --prefix value' >&2; exit 2; }; prefix=$2; shift 2 ;;
    --no-desktop) desktop=no; shift ;;
    --help) echo 'Usage: ./install.sh [--prefix PATH] [--no-desktop]'; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$prefix" in /*) ;; *) echo 'Prefix must be an absolute path' >&2; exit 2 ;; esac
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$prefix/bin" "$prefix/share/agentbanter/licenses"
install -m 755 "$source_dir/agentbanter" "$prefix/bin/agentbanter"
cp -R "$source_dir/licenses/." "$prefix/share/agentbanter/licenses/"
install -m 644 "$source_dir/LICENSE" "$prefix/share/agentbanter/LICENSE"
if [ "$desktop" = yes ] && [ "$(uname -s)" = Linux ]; then
  "$prefix/bin/agentbanter" desktop install
fi
printf 'Installed %s/bin/agentbanter\nRun: %s/bin/agentbanter open\n' "$prefix" "$prefix"
case ":${PATH}:" in *":$prefix/bin:"*) ;; *) printf 'Add %s/bin to your PATH for the short command.\n' "$prefix" ;; esac
