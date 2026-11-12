# rerecast 0.5.0 — patch do Viber

Cópia de `rerecast` 0.5.0 (crates.io, 2026-10-26, MIT OR Apache-2.0) ligada por
`[patch.crates-io]` no `Viber/Cargo.toml`. Só `src/poly_mesh.rs` e
`src/detail_mesh.rs` diferem do original; cada alteração está marcada com
`Viber patch:`.

1. **Triangulação falhada já não aborta a navmesh.** O Recast C++ devolve os
   triângulos feitos até ali (`return -ntris`) e o `rcBuildPolyMesh` só loga um
   warning — um contorno dobrado sobre si próprio deixa um buraco pequeno. O
   port Rust transforma isso em `PolygonNavmeshError::InvalidContour`, que
   deita fora a navmesh inteira. No `simple-rpg` o tile de 256 m do spawn
   falhava de forma determinística (qualquer deslocamento de grelha e qualquer
   `max_simplification_error`): nenhuma criatura tinha navmesh em toda a
   sessão.
2. **`len` das diagonais em `u32`.** O quadrado do comprimento era truncado com
   `as u16`: qualquer diagonal com mais de 255 células dava a volta e o ear
   clipping escolhia orelhas arbitrárias (triângulos compridos e finos). Com
   tiles de 256 m e células de ~0,5–1 m, isto é o caso normal, não a exceção.

Verificado a 2026-11-11: o upstream **0.5.0** (a versão publicada depois do
Bevy 0.20) continua com `as u16` e com o `Err(InvalidContour)` — nenhum dos
dois fixes entrou. Remover quando o upstream aceitar o equivalente (e apagar a
entrada `[patch.crates-io]`).

Deltas de vendor em relação ao crate publicado:

* `bevy_reflect` 0.20.0 → **0.20.1** e glam **0.33.2** (a versão do
  `bevy_math` 0.20.1; sem patch de git, o `^0.33` upstream já resolveria
  igual);
* `tests/` e as dev-dependencies removidas (nunca se constrói os testes do
  fork e eram elas que reancoravam o lock).

3. **Orientação do detail mesh garantida** (`src/detail_mesh.rs`,
   `clockwise_xz`). A triangulação do detail mesh deixa às vezes um sliver
   quase colinear com a orientação trocada por ruído de f32; o `landmass`
   valida todos os triângulos (`The triangle at index N in the height mesh is
   clockwise instead of counter-clockwise`) e deita fora a navmesh do tile
   INTEIRO — no Vael (2026-11-12) um tile ficava sem navegação em todas as
   sessões. O triângulo trocado vira-se (B↔C, flags AB↔CA).
4. **Triângulos invertidos fora da poly mesh** (`src/poly_mesh.rs`,
   `wound_right`). Com a triangulação recuperada (patch 1 e o diagonal
   "loose"), um triângulo invertido fundia-se num polígono côncavo e o
   `landmass` rejeitava o tile (`The polygon at index N is concave or has
   edges in clockwise order`). Descarta-se só o invertido; os colineares
   ficam para não abrir T-junctions.
