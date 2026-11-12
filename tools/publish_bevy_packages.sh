#!/usr/bin/env bash
# Publica os pacotes `bevy_*` na ordem de dependência.
#
#   tools/publish_bevy_packages.sh --dry-run   # valida os três sem publicar
#   tools/publish_bevy_packages.sh             # publica (precisa de token)
#
# O `bevy_save_slots` depende de `bevy_world_xml`; o script espera o índice
# do crates.io antes do terceiro publish. O token vem de `cargo login`
# (~/.cargo/credentials.toml) ou de CARGO_REGISTRY_TOKEN.
set -euo pipefail

cd "$(dirname "$0")/.."

mode="${1:-publish}"
if [[ "$mode" != "--dry-run" && "$mode" != "publish" ]]; then
  echo "uso: $0 [--dry-run]" >&2
  exit 2
fi

if [[ "$mode" == "publish" ]]; then
  if [[ -z "${CARGO_REGISTRY_TOKEN:-}" && ! -f "${CARGO_HOME:-$HOME/.cargo}/credentials.toml" ]]; then
    echo "sem token do crates.io: corre 'cargo login' ou define CARGO_REGISTRY_TOKEN" >&2
    exit 1
  fi
fi

publish() {
  if [[ "$mode" == "--dry-run" ]]; then
    cargo publish --dry-run --locked "$@"
  else
    cargo publish --locked "$@"
  fi
}

echo "== bevy_engine_core =="
publish -p bevy_engine_core

echo "== bevy_world_xml =="
publish -p bevy_world_xml

if [[ "$mode" == "--dry-run" ]]; then
  # O dry-run do save_slots só resolve depois de world_xml estar no registry;
  # em dry-run local validamos o conteúdo do pacote.
  echo "== bevy_save_slots (package --list; publish dry-run só após publicar world_xml) =="
  cargo package --list -p bevy_save_slots
  exit 0
fi

echo "== à espera do índice para bevy_world_xml =="
for _ in $(seq 1 30); do
  if cargo search bevy_world_xml --limit 1 2>/dev/null | grep -q '^bevy_world_xml'; then
    break
  fi
  sleep 10
done

echo "== bevy_save_slots =="
publish -p bevy_save_slots

echo "OK: bevy_engine_core, bevy_world_xml e bevy_save_slots publicados."
echo "Sugestão: tag v0.1.0 do commit de release."
