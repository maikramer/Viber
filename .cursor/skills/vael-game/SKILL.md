---
name: vael-game
description: Usar quando o trabalho na engine Viber é sobre o jogo jogável. O mundo, os scripts e o pool estão no checkout irmão ../Vael.
---

# Jogo Vael

O RPG não vive neste repo. Checkout irmão: `../Vael` (`world.xml`,
`scripts/`, `quests/`, `scenes/`, pool em `assets/`).

```bash
cargo run -- run ../Vael/world.xml
cargo run -- analyze ../Vael/world.xml
cargo run --manifest-path editor/Cargo.toml -- ../Vael
```

Testes que leem o mundo usam `VAEL_ROOT` (default `../Vael`) e saltam
se o checkout não estiver ao lado. A demo de corrida fica em
`examples/simple-racer` com o pool mínimo `examples/racer-assets`.
