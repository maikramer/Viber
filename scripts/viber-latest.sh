#!/usr/bin/env bash
# `viber` sempre no código mais recente do checkout — sem pagar o cargo à toa.
#
# Instalado como ~/.local/bin/viber (ver o fim deste ficheiro). Cada chamada
# tira um fingerprint das FONTES que entram no binário (caminho + tamanho +
# mtime de src/, vendor/, assets/fonts/, Cargo.toml, Cargo.lock; ~15 ms) e:
#
#   * igual ao do último build bom  → corre o binário em cache, sem cargo;
#   * igual ao do último build FALHADO → a árvore não mudou desde o erro:
#     não recompila outra vez, corre o último binário bom;
#   * diferente → `cargo build`; sucesso atualiza a cache, erro fica
#     registado e corre o último binário bom.
#
# O build nunca deixa o jogo sem engine: uma árvore a meio de uma edição
# (outro agente a trabalhar) cai sempre no último binário que compilou.
#
# `debug` e `session` nunca compilam (clientes do bridge de uma engine já
# viva — um rebuild a meio de uma sessão de QA não serve a ninguém).
#
# Env:
#   VIBER_NO_BUILD=1     salta a verificação e corre a cache
#   VIBER_FORCE_BUILD=1  ignora os fingerprints e chama o cargo
#   VIBER_PROFILE=debug  perfil dev (compila mais rápido, corre mais lento)
#   VIBER_CHECKOUT=…     caminho do checkout
#
# Instalar:  install -m 755 scripts/viber-latest.sh ~/.local/bin/viber
set -uo pipefail

checkout="${VIBER_CHECKOUT:-/media/maikeu/b1e73891-ddde-49a0-9382-903accb68b49/GitClones/Viber}"
profile="${VIBER_PROFILE:-release}"
store="${XDG_DATA_HOME:-$HOME/.local/share}/viber/bin"
last_good="$store/viber-$profile"
ok_fp_file="$store/viber-$profile.ok"
fail_fp_file="$store/viber-$profile.fail"

# Tudo o que o `cargo build` do crate `viber` lê. Um `include_bytes!` novo de
# fora destas pastas tem de entrar aqui.
fingerprint() {
  (
    cd "$checkout" &&
      {
        echo "profile=$profile"
        find src vendor assets/fonts Cargo.toml Cargo.lock -type f \
          -not -path '*/__pycache__/*' -printf '%p %s %T@\n' 2>/dev/null | LC_ALL=C sort
      } | sha1sum | cut -d' ' -f1
  )
}

build=1
case "${1:-}" in
  debug | session | -h | --help | -V | --version | help | "") build=0 ;;
esac
[[ -n "${VIBER_NO_BUILD:-}" ]] && build=0

if [[ "$build" == 1 ]]; then
  fp="$(fingerprint)"
  ok_fp="$(cat "$ok_fp_file" 2>/dev/null)"
  fail_fp="$(cat "$fail_fp_file" 2>/dev/null)"
  if [[ -z "${VIBER_FORCE_BUILD:-}" && -x "$last_good" && "$fp" == "$ok_fp" ]]; then
    : # fontes iguais às do binário em cache
  elif [[ -z "${VIBER_FORCE_BUILD:-}" && -x "$last_good" && "$fp" == "$fail_fp" ]]; then
    echo "viber: o checkout não mudou desde o último build falhado — a correr o último binário bom" >&2
  else
    echo "viber: fontes da engine mudaram — a recompilar ($profile)…" >&2
    cargo_args=(build --manifest-path "$checkout/Cargo.toml" --bin viber)
    [[ "$profile" == release ]] && cargo_args+=(--release)
    if cargo "${cargo_args[@]}" >&2; then
      built="$checkout/target/$profile/viber"
      mkdir -p "$store"
      if ! cmp -s "$built" "$last_good" 2>/dev/null; then
        # cópia + rename: substituir no sítio um binário em execução dá ETXTBSY.
        cp "$built" "$last_good.tmp.$$" && mv -f "$last_good.tmp.$$" "$last_good"
      fi
      # O fingerprint é o de ANTES do build: uma edição feita durante a
      # compilação muda-o e o próximo arranque recompila.
      echo "$fp" >"$ok_fp_file"
      rm -f "$fail_fp_file"
    else
      mkdir -p "$store"
      # Fingerprint de DEPOIS: o próprio cargo pode reescrever o Cargo.lock,
      # e com o de antes o arranque seguinte voltava a tentar o mesmo erro.
      fingerprint >"$fail_fp_file"
      echo "viber: o build do checkout falhou — a correr o último binário bom ($last_good)" >&2
    fi
  fi
fi

if [[ ! -x "$last_good" ]]; then
  echo "viber: não há binário em $last_good — o checkout tem de compilar uma vez" >&2
  exit 1
fi
exec "$last_good" "$@"
