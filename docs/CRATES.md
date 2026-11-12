# CRATES.md — adoção de crates e watch-list

Estudo do ecossistema crates.io/Bevy feito a 2026-09-07 (actualizado no upgrade
a 0.20 a 2026-09-25) e o que dele entrou na engine. Critérios de adoção:
(1) compatibilidade Bevy confirmada nas
dependências publicadas, (2) substituir uma roda própria SEM mudar
comportamento visível (contrato "mesma seed, mesmo mundo"), (3) risco de
driver NV (materiais/shaders custom) avaliado, (4) manutenção ativa do crate.

> O plano de dividir a engine em pacotes `bevy_*` publicáveis (fases, DAG,
> ordem de publicação e gates) está em
> [`../../Vael/docs/PACKAGES.md`](../../Vael/docs/PACKAGES.md). Este
> documento continua a ser o registo de adoção de dependências da engine.

## 2026-11-11 — des-vendorização da navegação

Os três crates de navegação saíram no crates.io com Bevy 0.20 em
2026-10-26/27 e já não são vendorizados:

| Crate | Versão | Antes |
|---|---|---|
| `bevy_rerecast` | 0.6 (registry, `default-features = false`, `bevy_mesh`) | fork 0.5 em `vendor/` |
| `bevy_landmass` | 0.13 (registry) | fork 0.13 em `vendor/` |
| `landmass_rerecast` | 0.4 (registry) | fork 0.3 em `vendor/` |

O `rerecast` 0.5 continua a ser o **único** `[patch.crates-io]`: o registry
ainda não tem os dois fixes de robustez de `vendor/rerecast/VIBER_PATCH.md`
(triangulação falhada não aborta a navmesh inteira; diagonais em `u32` em vez
de `u16`). Remover o vendor e o patch quando os fixes entrarem upstream. A
adaptação de API vive em `src/nav/backend.rs`; o gate é a navmesh do `main`
e `cargo tree -d` sem duplicar Bevy/glam.

## 2026-11-12 — primeiros pacotes `bevy_*`

A Fase 1 começou a extrair o monólito (plano em
[`../../Vael/docs/PACKAGES.md`](../../Vael/docs/PACKAGES.md)):

| Pacote | O que saiu do root |
|---|---|
| `bevy_engine_core` 0.1.0 | `rng`, `script_kind`, `PlayerBody`, `MenusOpen`, `ScriptToast`, `AudioMixerSettings`, `UiSet`, `TerrainQuery`/`NavQuery` |
| `bevy_world_xml` 0.1.0 | `XmlNode`/`XmlDocument`, parser tolerante, `values`, `include`, `emit`, `project`/`root_of`, `vinst`, `PlacedInstance`, `DeltaPatch` |
| `bevy_save_slots` 0.1.0 | `SaveGame`, paths/slots, `capture`, `save_to_disk`/`load_from_disk`; o `SavePlugin` e a aplicação ao runtime ficam no root |

O root `viber` re-exporta os tipos e mantém as paths antigas; os três
crates são os publicáveis (`cargo publish --dry-run` como gate).
`bevy_audio_buses` fica para a onda seguinte: `music`/`ambient` dependem de
`player`/`worldsys`/`particles`/`editor_camera`/`luau::sfx`; mover os
ficheiros arrastaria o runtime. O próximo passo é inverter essas
dependências, não mover o código como está.

## Adotados nesta ronda

| Crate | Versão | Substitui | Notas |
|-------|--------|-----------|-------|
| `dirs` | 7.0 (2026-10-18; era 6.0) | `HOME`/`XDG_CACHE_HOME` manuais | `save.rs` usa `home_dir()` (semântica `$HOME` exata — saves históricos em `~/.local/share/viber`); `session.rs` usa `cache_dir()`. |
| `half` | 2 | `half_to_f32` manual (`heightmap.rs`) | Conversão f16→f32 bit a bit idêntica. |
| — (dedup) | — | SplitMix64 ×6 ficheiros | `src/rng.rs` único; golden test congela a sequência (`Rng::new(0)` → `0xE220A8397B1DCDAF…`). `spawner.rs` re-exporta para compat. |
| `notify` | 8.2 | — (feature nova) | Hot-reload de scripts Luau: `src/hot_reload.rs`, watcher recursivo sobre `<mundo>/scripts/`, debounce 250 ms, erro de compilação mantém o chunk antigo. `VIBER_HOT_RELOAD=0` desliga. |
| `bevy_kira_audio` | 0.27.1 (kira 0.12; era 0.26, depois rc.1) | `bevy_audio`/rodio como backend de playback | Buses tipados `MusicBus`/`SfxBus` (`crate::music`); `mixer_sync` empurra o `AudioMixerSettings` (save/menu/XML) para os canais — volumes respondem AO VIVO, mesmo em one-shots a meio. Crossfade `fade_step` preservado em linear (testes intactos); conversão `linear_to_db` só na fronteira. Jitter de pitch via `with_playback_rate`. Cachoeiras/água/chuva/BGM todos no bus SFX/Music. |
| `serde_yaml` | 0.9 | parsing do `config.yaml` do jogo (`src/config.rs`) | Contrato de paths lido em runtime (roots/bgm/sfx/texturas/scripts/ui/save). Arquivada (dtolnay) mas congelada-estável e é o parser de referência; ~1 dep transitiva (`unsafe-libyaml`). Chaves desconhecidas avisam e não partem o parse. |
| `bevy_gradmesh` | **=0.3.7** (pin exato) | `Mesh::from(Cuboid/Sphere/Cylinder/Capsule)` + `StandardMaterial` nas primitivas de `<Composition>` | Meshes watertight com `ShapeDetail` e `GradMeshMaterial` (ExtendedMaterial PBR). `recipes::spawn` converte `Box`/`Sphere`/`Cylinder`/`Capsule`; `Plane` fica no StandardMaterial (decals/soalhos texturados). Os colisores Rapier não mudam. **O material gradiente não amostra texturas**: albedo/normal são ignorados e `texture-tile-size` vira ruído (`gradmesh_style`); o fecho do spawn avisa os URLs ignorados. `VIBER_PRIMITIVES=standard` volta ao caminho antigo; `VIBER_PRIMITIVES_DETAIL=minimal|low|medium|high|extreme` escolhe o detalhe (default `high`, 24 segmentos). Pin exato porque o crate teve 25 releases em 13 dias (0.3.x) — subir é decisão explícita. |
| `dlss_wgpu` | 6.0.0 (via `bevy/dlss`, feature `dlss` do Viber) | — (feature nova) | DLSS Super Resolution nativo do Bevy 0.20 (Linux/Vulkan). **NÃO-default**: o build com `--features dlss` exige o SDK da NVIDIA (`DLSS_SDK` → clone de github.com/NVIDIA/DLSS tag v310.9.1; `VULKAN_SDK` com headers + libclang para o bindgen) e em runtime a `libnvidia-ngx-dlss.so.310.9.1` junto ao binário (rpath `$ORIGIN` no `.cargo/config.toml`). Gate em `src/dlss.rs` (`VIBER_DLSS` > config.yaml `dlss:` > auto). Pin `prettyplease` 0.2.37 RETIRADO (2026-10-18): o lock resolve `bindgen 0.73.2` + `prettyplease 0.3` sem âncora. |

O `bevy_audio`/rodio do Bevy fica compilado (tirar a feature do `bevy` exige
re-listar ~20 features default — frágil); NENHUM som nasce por ele —
`AudioPlayer`/`PlaybackSettings` deixaram de ser usados em runtime.

### `bevy_gradmesh`: limites da v1

O material gradiente calcula o progresso do gradiente relativo a
`world_center` (uniforme do material, default a origem do mundo). Num objeto
longe da origem — o caso do Vael, a ~40–140 m de cota — um `Style::shaded`
ficaria todo no extremo claro/escuro. Por isso a v1 usa **cor única**
(`opaque`/`transparent`) com os campos PBR preservados; o ruído/stripes do
crate (que usam o espaço local da entidade) continuam disponíveis e são o que
substitui o grão do albedo perdido. Para ligar gradientes por objeto seria
preciso um `GradMeshMaterial` por entidade com `with_gradient_bounds` — fica
como follow-up (custa o dedup de materiais e o batching).

`GradMeshMaterial` translúcido continua a desenhar no passe de sombra (só o
alpha-test corta), por isso as primitivas com `opacity < 1` levam
`NotShadowCaster`, como manda o contrato do crate.

## Forks vendored para o Bevy 0.20 (2026-09-25; bevy oficial 2026-10-09)

O Bevy **0.20.0 final** está no crates.io desde 2026-10-08 — o `bevy = "0.20"`
resolve do registry e a tabela `[patch.crates-io]` de tags git foi removida.
Os crates do ecossistema abaixo continuam em `vendor/` até os upstreams
publicarem as versões bevy-0.20 (proveniência/revs e condições de des-vendor
em [`vendor/FORKS_020.md`](../vendor/FORKS_020.md)):

| Fork | A montar quando | Notas do port |
|------|-----------------|---------------|
| ~~`bevy_rapier3d` 0.36~~ | **DESVENDORED 2026-10-18** — o upstream `dimforge/rapier` publicou o **0.37.0** (bevy ^0.20, rapier3d 0.36.1, nalgebra 0.35 convert-glam033), que é o port oficial daquilo que o fork fazia à mão | `bevy_rapier3d = { version = "0.37", default-features = false }` do registry |
| `bevy_rerecast` + `bevy_rerecast_core` 0.5 | `janhohenheim/rerecast` publicar 0.20 (a 2026-10-18 o 0.5.0 continua a última versão) | reflect derives por feature do `rerecast` core. |
| `bevy_landmass` 0.13 + `landmass_rerecast` 0.3 | `andriyDev/landmass` branch bevy-0.20 virar release (a 2026-10-18 o 0.12.1/0.3.0 do crates.io continuam bevy ^0.19) | landmass_rerecast ainda misturava bevy 0.19 no branch — corrigido no vendor. |

Único `[patch.crates-io]` restante: `rerecast` (fixes locais de robustez —
`vendor/rerecast/VIBER_PATCH.md`). Dev-deps de shader alinhados com o bevy 0.20
(2026-10-18): `wesl` 0.6 + `naga` 30 — UMA só cópia de cada no lockfile
(antes: wesl 0.4.2 + naga 29.0.4 duplicados na árvore); com o parser do wesl 0.6
a aceitar `binding_array`, os 2 testes ignorados de `tests/chunk_shader.rs`
foram reativados (stub `bevy_render::bindless` com o `enable wgpu_binding_array;`
e `VertexOutput.instance_index` com `@interpolate(flat)`, como no
`bevy_pbr`/`bevy_render` reais).

## Ronda de atualização 2026-10-18

* `bevy_rapier3d` 0.36 (fork) → **0.37.0** (registry) — ver acima.
* `bevy_kira_audio` 0.27.0-rc.1 → **0.27.1** (stable, kira 0.12 sem mudança).
* `mlua` 0.10 → **0.12**: `StdLib::PACKAGE` deixou de existir para Luau
  (`ALL_SAFE` = todas as libs do Luau — sem package/os/io de qualquer forma) e
  `Table::set_metatable` devolve `Result` (propagado nos 3 sítios: host.rs,
  game.rs, bridge/lua.rs).
* `dirs` 6 → **7**, `meshopt` 0.6 → **0.7**, `base64` 0.22 → **0.23**,
  `roxmltree` 0.20 → **0.21** — drop-in (zero alterações de código).
* Lock: `prettyplease` 0.2.37 → **0.3.0** com `bindgen` 0.73.2 como único
  dependente — o pin do build DLSS deixou de ser necessário. O mecanismo: o
  bindgen 0.73.2 declara `prettyplease >=0.2.7, <0.4` e `syn >=2,<4` — NESTE
  lock liga `syn 3.0.9` + `prettyplease 0.3` (o par consistente). Se uma
  dependência futura re-ancorar o syn do bindgen em 2, repor o pin com
  `cargo update -p prettyplease --precise 0.2.37` (o falhanço sem pin é um
  `syn::file::File: ParseQuote` dentro do bindgen, só no build `--features dlss`).

## Ronda de atualização 2026-11-07

* **`bevy` 0.20.0 → 0.20.1** (patch, crates.io 2026-10-28) no engine e no
  editor; `rust-version` sobe 1.96/1.97 → **1.97.1** (o MSRV real do 0.20.1).
  Zero alterações de API no nosso código: `cargo check --workspace
  --all-targets` limpo à primeira. O patch traz 9 fixes: `ruzstd` 0.8→0.9,
  `UninitBufferVec` a usar o size por entrada do shader, `ReflectDefault`
  para `Option<T>`, evicção de entidades órfãs na travessia de ancestrais,
  unicidade nos iteradores paralelos, dependência cíclica de shaders em
  `bevy_render` e fog volumes a desaparecer perto do near-plane.
* Manifests vendorizados: os pins `0.20.0-rc.2` → **`0.20.1`** (38 linhas nos
  5 forks). Era um requisito pré-release que já resolvia para 0.20.x; passa a
  dizer a versão que testamos.
* `cargo update` das compatíveis: 139 pacotes (`icu_*` 2.3→2.4, `toml`
  1.1→1.2, `notify` 8.2.1, `roxmltree` 0.21.2, `flate2` 1.1.11, `uuid`
  1.29.1, `kira` 0.12.6, `ruzstd` 0.9.1, `tokio` 1.54.1, …). As deps
  directas já estavam na última: `bevy_rapier3d` 0.37.0,
  `bevy_kira_audio` 0.27.1, `mlua` 0.12.2, `image` 0.25.10, `dirs` 7.0.0,
  `clap` 4.6.7. As 4 "behind latest" que o `cargo update` reporta são
  transitivas presas por semver (`i_float`/`i_overlay`/`i_shape` do rapier,
  `lua-src` do mlua).
* **`glam` ganha uma 6.ª cópia (0.34.2)** via `kira 0.12.6`: o kira subiu o
  glam interno e bevy/`bevy_math` continuam em 0.33.12 (`bevy_math 0.20.1`
  declara `glam ^0.33.2`). A fronteira é interna do kira (spatial audio): o
  `bevy_kira_audio` não expõe tipos glam ao nosso código e o workspace
  compila.

### Achados da ronda (pré-existentes, não bloqueiam o upgrade)

* `tests/collider_bake.rs` pinava 163 vértices / 966 índices / 322 triângulos
  do `market_stall_collision.glb`; a regen do pool (Vael `3c711b5`,
  2026-11-06) deixou o asset em 162/960/320. Pins actualizados para o asset
  vigente.
* `luau::decision::tests::lua_api_intent_pt_multi_question` falha de forma
  intermitente (~1 em 3 corridas paralelas) com `only one environment is
  allowed per process` do ONNX Runtime: `celer-decision` e `celer-nlu` criam
  um `Ort::Env` por `Runtime`, e dois runtimes vivos no mesmo processo
  colidem. O teste ainda mascarava o erro (`kind="loaded"` com `ok=false`) —
  passou a assertar `ok` e a mostrar o erro real. Fix de raiz: env
  partilhado (`OnceLock`) nos crates do CelerAI.
* `bridge::tests::test_bridge_end_to_end_headless` continua flaky sob carga
  (a evicção do duplo buffer de `Messages<CursorMoved>` corre com o
  `click`+`move` injectados) — passa isolado e na maioria das corridas.

## Follow-ups da ronda (veredictos ao vivo, 2026-10-18)

* **Luau JIT ADOTADO** — feature cargo `luau-jit` (mlua 0.12, codegen do Luau,
  vector_size 3 do `luau0-src`) no default; `VIBER_LUAU_JIT=0` volta ao
  interpretador no MESMO binário. Medido ao vivo (REPL, 10M iterações de loop
  numérico quente): ~1,17× (0,196 s vs 0,235 s). O orçamento de 500 ms por
  chamada interrompe TAMBÉM código JIT — `test_infinite_loop_is_interrupted_by_the_cpu_budget`
  passa com a feature ligada (67/67 testes luau verdes).
* **Re-upload de uniforms de materiais custom: CONTINUA PARTIDO** — o motivo
  de o vento da relva/céu/água viverem em consts no fonte do shader foi
  re-verificado ao vivo (sonda `#[uniform(100)]` da `GrassExtension` mutada em
  onda quadrada por `Assets::get_mut`, 28 tiles de relva vivos no campo da
  câmara): a relva NÃO respondeu ao longo de 2 períodos — só o upload inicial
  chega à GPU. Veredicto anotado em `src/grass.rs` (`configure_shader`).
* **SIGSEGV bindless: ENCERRADO** — o crash de `vkCreatePipelineLayout`
  (`libnvidia-gpucomp`) com materiais custom não-bindless + `#[texture]`
  JÁ NÃO REPRODUZ no bevy 0.20 + wgpu 30 + driver NV 615.71.09 (o gatilho era
  wgpu 29 + driver 595.84). Experimento: `#[bindless]` do
  `TerrainChunkMaterial` removido → boot completo de `worlds/lua-demo` com
  terreno desenhado (look flat). Bindless mantido pelo layout, não por
  necessidade — veredicto em `layer_material.rs` e `chunk.wesl`.
* **`ui_focus_system` portado: MIGRADO AO PICKING NATIVO (2026-10-18)** — o
  port do `ui_focus_system` 0.19 saiu; [`src/ui/interaction.rs`](../src/ui/interaction.rs)
  é agora um adaptador fino: o hit-testing é o `bevy_ui::picking_backend`
  (mesma travessia UiStack/clip/visibilidade, truncada no 1.º bloqueador) e o
  adaptador traduz a cadeia `PointerHits` para a `Interaction`/`Button`
  nuestras com a semântica de sempre (os ~12 ficheiros consumidores não
  mudaram). O blocking vive em `Pickable` (`runtime::ui_pickable` traduz a
  política; o backend ignora `FocusPolicy`). Bónus de perf: o mesh picking
  (sem consumidor na engine) ficou `require_markers: true` — deixou de
  raycastar todas as malhas visíveis por frame. QA ao vivo: tooltip de hover,
  hover de aba e TROCA DE ABA POR CLIQUE (Loja aberta no menu Diário) com o
  binário novo. Nota de QA: clique sintético no MESMO frame do `move` apanha
  a posição ANTIGA (o backend de picking lê o cursor no `First` do frame
  seguinte) — a receita continua sendo `viber debug move X Y` → esperar →
  `click X Y`.

## Watch-list (avaliar numa próxima ronda)

Confirmados compatíveis com Bevy 0.19 (dependências publicadas verificadas a
2026-09-07):

- **`bevy_rerecast` + `vleue_navigator`** — port do Recast (navmesh a partir
  da geometria real: trimesh das colunas + colliders) + pathfinding Polyanya.
  Usaria a tag `NavMesh` (hoje só afina o landmass) e substituiria o
  wander/chase determinístico da `src/ai.rs` quando o mundo pede navegação.
  PRÓXIMO CANDIDATO.
- **`bevy_replicon`** 0.44 (set 2026) — replicação server-authoritative;
  **`lightyear`** 0.29 — alternativa com prediction/interpolation. Multiplayer
  = decisão arquitetural grande: spike documentado antes de integrar.
- **`leafwing-input-manager`** 0.21 — ações + rebinding de teclas (o usuário
  remapear WASD; hoje o mapa é fixo em `player.rs`).
- **`bevy-inspector-egui`** 0.37 (+`bevy_egui` 0.42) — inspeção live de
  componentes para debug; sobrepõe-se ao `viber.debug.*`/profiler? Decidir
  antes de trazer egui para a stack (peso de build).
- **`bevy_sonus`** — áudio espacial COM oclusão/difração — encaixe perfeito
  para as caves voxel; crate novo, vigiar maturidade. O bus kira atual já
  dá o caminho (spatial é por-instância).
- **`bevy_hanabi`** 0.20.0 — partículas GPU (fogo/chuva/magia em milhares).
  Compatível com Bevy 0.20/wgpu 30; o protótipo do Vael vive em
  `Vael/src/hanabi_fx.rs` (feature `hanabi`, `VAEL_HANABI=1`). Não substitui
  o sistema CPU do Viber sem A/B; ⚠ histórico de crash do driver NV 595.84
  com shaders custom (`docs/PERFORMANCE.md`).
- **`bevy_mod_outline`** 0.14.0 — outline de seleção/interatuáveis;
  compatível com `bevy ^0.20.1`. Watch-list: testar TAA/skinned antes de
  adotar; a seleção do editor continua por AABB+Rapier.
- **`bevy_sonus`** 0.1.0 — incompatível (Bevy 0.19, puxa rodio); rever se
  houver versão 0.20/kira.

## Ronda 2026-11-11 — Bevy 0.19/0.20 e o refino do Vael

- **`area_light_luts`** (feature do `bevy`, não crate): adotada para
  `RectLight` nos interiores do Vael. LUT embutida, sem textura/shaders
  próprios; os testes de bindless do terreno continuam a ser o gate.
- **`bevy_hanabi` 0.20.0**: protótipo feature-gated no Vael; não entra no
  default nem substitui o sistema CPU.
- **`bevy_mod_outline` 0.14.0**: watch-list, não adotado.
- **`bevy_sonus` 0.1.0**: rejeitado (Bevy 0.19/rodio).
- **`bevy_tnua` / `bevy_camera_controller` / `bevy_water` / `bevy_framepace`**:
  rejeitados; o Viber já tem implementações mais específicas.
- Detalhe e justificação: [`docs/BEVY_0.19_0.20.md`](BEVY_0.19_0.20.md).

## Rejeitados / não adotar sem reavaliar

- **`bevy_save`** 2.0.1 — STALE (última publicação ago 2025, pré-0.19). O
  `save.rs` custom (JSON + tmp+fsync+rename atómico, campos `#[serde(default)]`)
  é superior para o caso: manter.
- **`rand`/`noise` para os algoritmos de mundo** — trocar SplitMix64/value-noise
  FBM por crates QUEBRA o contrato "mesma seed, mesmo mundo" (mundos
  procedurais mudariam de forma). Os algoritmos estão congelados em
  `src/rng.rs` + `heightmap.rs` com golden tests.
- **`Dexterous Developer`** — hot-reload pesado de código; para scripts Luau
  o `notify` é suficiente (adotado).
- **`quick-xml`/`roxmltree` swap, tokio no bridge, `ureq` no cliente HTTP** —
  `roxmltree` já usado; o cliente std-only do bridge (`src/bridge/client.rs`)
  é decisão consciente (CI-ready, zero deps) — só rever se precisar de TLS.
