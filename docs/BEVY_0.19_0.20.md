# Bevy 0.19/0.20 — auditoria para o Vael

Estado: 2026-11-11. O Viber está em **Bevy 0.20.1** (wgpu 30, WESL 0.6) e o
Vael usa o mesmo `bevy` por path-dep. Esta nota regista o que o 0.19/0.20
trouxe, o que já usamos, o que está bloqueado pelo renderer forward/bindless e
o que vale a pena experimentar. Não é um changelog exaustivo: é a decisão de
adoção para este jogo.

## Já usado (não reimplementar)

- `VisibilityRange` para o cull de distância (`Viber/src/render_lod.rs`).
- GPU frustum culling, GPU visibility-range checks, clustering de luzes/probes
  e bounds de skinned mesh: automáticos no caminho Vulkan.
- `LightProbe` + `GeneratedEnvironmentMapLight` com
  `ParallaxCorrection::Auto` (`Viber/src/probes.rs`, `src/ibl.rs`).
- WESL nos shaders de terreno/água/céu/SSR (`*.wesl`, `wesl = 0.6`).
- `dfg_lut`, `bluenoise_texture`, `experimental_pbr_pcss`.
- `bevy_remote`, `bevy_dev_tools`, `bevy_gizmos`, `bevy_ui_widgets`,
  `bevy_input_focus`, `bevy_picking` (UI).
- TAA, SSAO, bloom, auto-exposure, PCSS, color grading, CAS; vinheta e
  aberração cromática existem como componentes do Bevy, mas o Vael desliga-as
  por decisão de gosto.
- `bevy_kira_audio` para buses/ducking/reverb (o `bevy_audio` não tem buses).

## 0.19 — o que interessa

| Feature | Estado no Viber/Vael | Decisão |
|---|---|---|
| Contact shadows | Componente já criado em `postfx.rs`; o Vael tinha-o desligado. Custo medido: ~1,5 ms no sol; 18–21 ms se aplicado a point lights. | **A/B ligado só no sol**; point lights continuam off. |
| Rectangular area lights (`RectLight`, `area_light_luts`) | Não existia parser/spawn. | **Adotado** para fill de interiores (forge/barn/market/longhouse), sem sombras. |
| Physically based SSR | `bevy_pbr::ScreenSpaceReflections` exige `DeferredPrepass`; o terreno bindless e a água são forward. | **Não adotar**. O caminho é o `water_ssr.rs` forward do Viber. |
| Parallax-corrected cubemaps | Já usado. | Sem ação. |
| GPU visibility-range / `NoCpuCulling` | `VisibilityRange` já; `NoCpuCulling` nunca usado. | **Piloto opt-in** (`VIBER_GPU_CULLING=1`) só em malhas estáticas; luzes/skinned ficam na CPU. |
| Skinned mesh culling | glTF já gera bounds animados; só malhas manuais precisam de `DynamicSkinnedMeshBounds`. | Sem ação. |
| Solari | Removido do Viber: ~6 ms/frame na RTX 4050 sem ganho visual, proxies para o terreno bindless. | **Não reavaliar** para este alvo. |
| Mesh shaders | Suporte base, sem integração de alto nível; não há web. | Adiar. |
| `DiagnosticsOverlayPlugin` | O F3 do Viber é game-aware (sistemas, scripts, chunks, partículas). | Não substituir. |
| `Vignette` / `LensDistortion` | Vinheta já é componente do Bevy, desligada no Vael; LensDistortion adiciona um passe. | P2 estético; não agora. |
| BSN / scenes | O Vael/Viber têm XML/`SceneDoc` próprio. | Não adotar nesta fase. |
| Resources as components | Não há API pública de autoria no 0.20.1. | Sem ação. |

## 0.20 — o que interessa

| Feature | Decisão |
|---|---|
| WESL | Já adotado. |
| `chain_weak`/`before_weak`/`after_weak` | Candidato pequeno no chain do `render_lod`; não é prioridade. |
| `#[component(summary_tick)]` | Sem query de alta cardinalidade que beneficie; não aplicar. |
| `despawn_all`/`despawn_all_where` | Só em `unload_scene`, que precisa de limpeza por entidade; micro-ganho. |
| Panic catching / `BevyError` | Não ligar em sistemas de engine que devem falhar rápido. |
| `bevy_camera_controller` (PanOrbit) | A câmara do editor é terrain-aware e dirigida pela bridge; não substituir. |
| `MeshPicking` | A seleção do editor usa Rapier como oclusor e mapeia nós XML; o mesh picking não tem isso. |
| `CompressedImageSaver` | O pipeline KTX2/UASTC+Zstd é próprio; não trocar. |
| DLSS | Já integrado como feature opcional (`--features dlss`); é o único item de 0.19/0.20 com ganho de performance claro. |
| Solari / mesh shaders | Fora de âmbito. |

## Crates.io — shortlist

| Crate | Versão | Compatibilidade | Decisão |
|---|---|---|---|
| `bevy_hanabi` | `=0.20.0` | `bevy ^0.20`, `wgpu ^30`; MIT/Apache | **Protótipo**: brasas nas fogueiras, feature `hanabi` + `VAEL_HANABI=1`. Não substitui o sistema CPU. |
| `bevy_mod_outline` | `=0.14.0` | `bevy ^0.20.1`; MIT/Apache | **Watch**: outline de interatuáveis; testar TAA/skinned antes de adotar. |
| `bevy_sonus` | 0.1.0 | `bevy ^0.19`; puxa rodio | **Rejeitado** enquanto não houver versão 0.20/kira. |
| `bevy_tnua` | 0.33 | Bevy 0.20 + Rapier 0.37 | **Rejeitado**: o `player.rs` tem coyote time, jump buffer, knockback e câmara; é reescrita sem ganho visual/perf. |
| `bevy_camera_controller` | 0.20.1 | Bevy 0.20 | **Rejeitado**: perde o alvo terrain-aware e o estado da bridge. |
| `bevy-atmosphere` | 0.13 | Bevy 0.16 | **Rejeitado** (stale); o céu é WESL próprio. |
| `bevy_water` | 0.20 | Bevy 0.20 | **Rejeitado**: oceano; a água do Vael é `ExtendedMaterial` + SSR forward. |
| `bevy_framepace` | 0.23 | Bevy 0.20 | **Rejeitado**: o pacer do Viber é mais fino (divisores, p90, spin). |
| `bevy_asset_loader` | 0.28 | Bevy 0.20 | **Rejeitado**: roots/spawn/streaming próprios. |
| `bevy-inspector-egui` | 0.38 | Bevy 0.20 | **Dev-only / watch**; não entra no build de jogo. |

## Implementação desta ronda

- `AppOptions::ecs_mirror` (default `true`; Vael `false`, override
  `VIBER_ECS_MIRROR=1`): o espelho refletido por frame de `viber.ecs` deixa de
  bloquear o `Update` no jogo.
- `run_if` nos sistemas exclusivos `apply_script_spawns` e
  `drive_scene_loader` (com `needs_tick`, para o contador de silêncio dos
  portais continuar a drenar).
- `range_scale` e os gates de partículas lidos uma vez por frame, não por
  entidade; `nearest_within` compara distância ao quadrado.
- Vael: snapshot de estado só aplicado quando muda, HUD sem republish por
  frame, cache de `QuestRules`, viagem só escreve snapshots diferentes.
- `streaming.rs`: chaves de hibernação em `HashSet` (remove O(n²)).
- `render_lod.rs`: scan da escada a 10 Hz (`VIBER_LOD_SCAN_HZ=0` desliga o
  throttle para A/B) e piloto `NoCpuCulling` (`VIBER_GPU_CULLING=1`, só
  malhas estáticas).
- `Vael/config.yaml`: contact shadows no sol e `reflections.mode: water`.
- `Vael/scenes/interiors/*.xml`: `RectLight` de janela/fogo ao lado das
  PointLights de teto. A calibração por screenshot mostrou que 3 000 lm é
  invisível e 100 000 lm estoura a exposição; forge/barn/longhouse ficaram em
  60 000 lm e market em 40 000 lm.
- `Vael/src/hanabi_fx.rs`: protótipo de brasas (feature `hanabi`).

## Estado de validação (2026-11-11/12)

- `cargo check --lib` do Viber: limpo com as alterações acima (incluindo
  `area_light_luts`).
- Testes do Viber num worktree limpo: `render_lod` 13/13 (o throttle passou a
  usar `Option<Res<Time>>` para apps mínimas), `streaming` 5/5,
  `particles` 23/23, `scene_loader` 6/6, `luau::ecs` 7/7 e o novo
  `test_rect_light_overrides` 1/1.
- `cargo check` do Vael num worktree limpo (HEAD + estes patches): passou; a
  árvore principal está temporariamente bloqueada pela WIP concorrente de
  `src/lighting.rs`/workspace `editor/content`, não por estes patches. O
  efeito Hanabi compilou isoladamente num crate mínimo com
  `bevy_hanabi 0.20.0`.
- QA live: o binário `target/release/vael` de 22:07 é anterior ao parser
  `RectLight`; precisa de rebuild. Com `VAEL_FLORA=0`, a vila arranca, gates
  `VAEL_*` ficam true e o HUD publica `100/100`/`30 de ouro`. Com flora ligada
  (WIP de outro agente) houve OOM de render aos ~38 s; não reproduzido com
  `VAEL_FLORA=0`.

## Como medir

```bash
# baseline / depois (mesmo local, hora e binário; janela visível)
viber debug prof --port 15706 --tab frames --samples 12
viber debug prof --port 15706 --hitches --hitch-ms 33
viber debug prof --port 15706 --record 30 --hz 2 -o /tmp/arm.jsonl

# A/B dos gates desta ronda
VIBER_LOD_SCAN_HZ=0 ...          # scan da escada por frame (controlo)
VIBER_GPU_CULLING=1 ...          # NoCpuCulling (Vulkan)
VIBER_NO_CONTACT_SHADOWS=1 ...   # desliga o contacto do sol
VIBER_WATER_SSR=0 ...            # desliga o SSR da água
VIBER_ECS_MIRROR=1 ...           # devolve o espelho de viber.ecs
```

Comparar sempre o mesmo local/hora e manter a janela visível. O `sched.last`
é o frame pacer a dormir; o trabalho real é a soma de `sched.*`.
