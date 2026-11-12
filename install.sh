#!/usr/bin/env bash
# Instala o CLI da engine em ~/.local/bin.
#
#   ./install.sh              # viber release
#   ./install.sh viber --debug
#
# `viber run` DENTRO deste checkout reconstrói via cargo. Fora dele (na pasta
# do Vael) corre o binário instalado aqui — por isso, depois de mudar a
# engine, volta a correr este script.
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)"
cd "$root"

what="viber"
profile="release"
for arg in "$@"; do
  case "$arg" in
    viber) what="$arg" ;;
    --debug) profile="debug" ;;
    -h|--help)
      echo "uso: ./install.sh [viber] [--debug]"
      exit 0
      ;;
    *)
      echo "argumento desconhecido: $arg" >&2
      echo "uso: ./install.sh [viber] [--debug]" >&2
      exit 2
      ;;
  esac
done

mkdir -p "${HOME}/.local/bin"
cargo_args=()
if [[ "$profile" == "release" ]]; then
  cargo_args+=(--release)
fi

case "$what" in
  viber)
    cargo build "${cargo_args[@]}"
    install -m 755 "target/${profile}/viber" "${HOME}/.local/bin/viber"
    echo "instalado: ${HOME}/.local/bin/viber ($(${HOME}/.local/bin/viber --version))"
    ;;
esac
