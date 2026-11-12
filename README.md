# Viber

Native Bevy 0.20 engine for declarative worlds (XML and Jackdaw `.bsn`):
terrain, navigation, physics, rendering, declarative UI, Luau scripting and a
debug bridge.

> **Estado:** o crate raiz `viber` é o monólito/app transitório
> (`publish = false`). A engine está a ser dividida em pacotes `bevy_*`
> publicáveis; o plano, o DAG e os gates estão em
> [`../Vael/docs/PACKAGES.md`](../Vael/docs/PACKAGES.md). O editor de
> autoria do jogo vive agora em [`../Vael/editor`](../Vael/editor) (Jackdaw);
> o Viber deixou de trazer um editor próprio.

## Pacotes extraídos

Vivem no monorepo irmão [`../bevy_extensions`](../bevy_extensions), com CI e
publicação próprios:

- `bevy_ext_core` — contratos (`PlayerBody`, `MenusOpen`, `ScriptToast`,
  `AudioMixerSettings`, `UiSet`, `rng`, `script_kind`).
- `bevy_ext_world` — XML/BSN/vinst, parser, `include`/`emit`,
  `project`/`root_of`, `DeltaPatch`.
- `bevy_ext_gltf` — decoder GLB (meshopt, `KHR_mesh_quantization`,
  `KHR_texture_basisu`, UASTC RRR), bounds e texturas tiled.
- `bevy_voxel_terrain` — núcleo voxel/SDF (mods CSG, caves, arcos, pontes,
  Transvoxel) e o trait `HeightField`.
- `bevy_ext_terrain` — contrato de cena do voxel e adapter `.ahgt`.
- `bevy_ext_save` — `SaveGame`, slots, IO atómico e migração legacy; o
  `SavePlugin` e a aplicação ao runtime ficam no root.

O root `viber` consome-os por path com os aliases históricos
(`bevy_engine_core`/`bevy_world_xml`/`bevy_save_slots`) e re-exporta as
paths antigas (`viber::xml`, `viber::save`, …). A publicação e o CI dos
pacotes vivem no monorepo; aqui fica só o `cargo package --list` do root.

## Correr

```bash
# a partir deste checkout
cargo run --release -- run ../Vael/world.xml
cargo run --release -- analyze ../Vael/world.xml
```

O binário `viber` do PATH é o wrapper `scripts/viber-latest.sh`:
reconstrói quando as fontes mudam e reutiliza a cache caso contrário.
`VIBER_NO_BUILD=1` salta o build; `VIBER_FORCE_BUILD=1` força.

O editor de conteúdo do jogo não vive aqui: está em
[`../Vael/editor`](../Vael/editor) (Jackdaw). O Viber não traz editor.

## Features

- `default = ["experimental_pbr_pcss", "luau-jit"]` — engine sem o checkout
  irmão CelerAI.
- `ai` — liga `nlu` + `decision` (`celer-nlu`/`celer-decision`). O Vael liga
  esta feature.
- `nlu` / `decision` — runtimes de IA individuais.
- `dlss` — DLSS Super Resolution (exige SDK NVIDIA; não-default).

## MSRV

1.97.1 (o MSRV do Bevy 0.20.1).

## Licença

MIT OR Apache-2.0. Os textos estão em `LICENSE-MIT` e `LICENSE-APACHE`.
A fonte Cinzel embutida no HUD é OFL 1.1; ver `NOTICE` e
`assets/fonts/OFL-Cinzel.txt`.
