#!/usr/bin/env bash
# GC do target/: o cargo nunca apaga gerações velhas em deps/.
#
# Cada rebuild que muda o metadata-hash escreve `<unidade>-<hash>` novo e o
# anterior fica para sempre. Com o wrapper a reconstruir a cada mudança de
# fonte (vários agentes em simultâneo) e `cargo test` a correr a rodo, o
# target chegou a 200+ GB (2026-10-24; o episódio dos 123 GB de binários de
# teste, mitigado no Cargo.toml a 2026-10-13, nunca tinha sido limpo).
#
# Regras — só toca em unidades do workspace e no lixo derivado delas; as
# rlibs de crates.io são estáveis (bevy/ort/ash) e nunca são apagadas:
#   * deps/: mantém a geração mais recente de cada unidade do workspace
#     (`viber`, `vibereditor`, `libviber`, … e os alvos de tests/*.rs),
#     agrupada pelo mesmo `-<hash>`; apaga as gerações antigas — binários,
#     rlibs, .rmeta, .d e os .rcgu.dwo do split-debuginfo, que acompanham a
#     geração pelo mtime.
#   * incremental/: sessões cujo ficheiro mais recente tem mais de AGE dias
#     (o rustc mantém as 2 vivas; as velhas de hashes antigos ficavam).
#
# Apagar a geração errada custa no máximo uma recompilação — nunca corrói
# nada (o target é cache). Uso:
#   target-gc.sh <dir-target> [checkout] [--dry-run]
# Env: VIBER_TARGET_GC_AGE=3  (dias para as sessões incremental)
set -u

target="${1:?uso: target-gc.sh <dir-target> [checkout] [--dry-run]}"
checkout="${2:-$(dirname "$(dirname "$(readlink -f "$0")")")}"
dry=0
[[ "${3:-}" == "--dry-run" ]] && dry=1
age_days="${VIBER_TARGET_GC_AGE:-3}"
grace=120   # segundos de tolerância dentro da mesma geração

# Unidades do workspace: os dois bins + os alvos de teste (o nome do exe de
# teste é o do ficheiro tests/*.rs). Exemplos/benches não existem; se um dia
# existirem, entram aqui.
units="viber|vibereditor"
for f in "$checkout"/tests/*.rs "$checkout"/editor/tests/*.rs; do
  [[ -e "$f" ]] && units="$units|$(basename "$f" .rs)"
done

freed=0
count=0
drop() { # $1 = caminho; $2 = tamanho em bytes; $3 = 1 se é diretório
  freed=$((freed + $2))
  count=$((count + 1))
  if [[ $dry == 1 ]]; then
    if [[ "${3:-}" == 1 ]]; then
      echo "dry-run: apagaria $(du -sh "$1" 2>/dev/null | cut -f1) $1"
    else
      echo "dry-run: apagaria $(numfmt --to=iec --suffix=B "$2" 2>/dev/null || echo "${2}B") $1"
    fi
  elif [[ "${3:-}" == 1 ]]; then
    rm -rf -- "$1"
  else
    rm -f -- "$1"
  fi
}

for profile in debug release; do
  deps="$target/$profile/deps"
  if [[ -d "$deps" ]]; then
    while IFS= read -r stale; do
      [[ -n "$stale" ]] || continue
      drop "$deps/$stale" "$(stat -c %s "$deps/$stale" 2>/dev/null || echo 0)"
    done < <(
      find "$deps" -maxdepth 1 -type f -printf '%T@\t%f\n' \
        | awk -v units="$units" -v grace="$grace" '
          {
            file = $2; mtime = $1 + 0
            stem = file; sub(/^lib/, "", stem)
            # unidade = tudo antes de -<hash>; nomes de crate não têm traços
            # (viram _), por isso o primeiro traço separa sempre.
            unit = stem; sub(/-[0-9a-f][0-9a-f]+.*$/, "", unit)
            if (unit == stem) next
            rest = substr(stem, length(unit) + 2)
            # rest = <hash de 16 hex>[.rlib|.rmeta|.d|.dwo|.<cguid>.<w>.rcgu.dwo]
            hash = rest; sub(/\..*$/, "", hash)
            if (length(hash) != 16) next
            if (rest !~ /^[0-9a-f]+(\.[A-Za-z0-9_.]+)?$/) next
            if (unit !~ "^(" units ")$") next
            if (!(unit in best) || mtime > best[unit]) best[unit] = mtime
            n++; files[n] = file; ftime[n] = mtime; funit[n] = unit
          }
          END {
            for (i = 1; i <= n; i++)
              if (ftime[i] < best[funit[i]] - grace) print files[i]
          }'
    )
  fi

  # Sessões incremental velhas: a idade é a do ficheiro mais recente lá
  # dentro (o mtime do diretório não acompanha escritas em subdiretórios).
  inc="$target/$profile/incremental"
  [[ -d "$inc" ]] || continue
  now=$(date +%s)
  for d in "$inc"/*/; do
    [[ -d "$d" ]] || continue
    newest=$(find "$d" -type f -printf '%T@\n' 2>/dev/null | sort -n | tail -1)
    [[ -n "$newest" ]] || newest=0
    newest=${newest%.*}
    if (( now - newest > age_days * 86400 )); then
      drop "$d" "$(du -sb "$d" 2>/dev/null | cut -f1 || echo 0)" 1
    fi
  done
done

if (( count > 0 )); then
  printf 'target-gc: %s %d itens (%s)\n' \
    "$([[ $dry == 1 ]] && echo 'apagaria' || echo 'apagou')" \
    "$count" "$(numfmt --to=iec --suffix=B "$freed" 2>/dev/null || echo "${freed}B")"
fi
exit 0
