# vendor/ — patch do rerecast (Bevy 0.20)

**Atualização 2026-11-11**: os quatro forks restantes do ecossistema de
navegação saíram no crates.io com Bevy 0.20 e foram **removidos do vendor**:

| Fork removido | Substituído por | Verificação |
|---------------|-----------------|-------------|
| `bevy_rerecast` 0.5.0 | **0.6.0** (crates.io, 2026-10-26) | `default-features = false, features = ["bevy_mesh"]` (sem editor/debug) |
| `bevy_rerecast_core` 0.5.0 | **0.6.0** (transitivo) | bevy 0.20 + `bevy_shape` |
| `bevy_landmass` 0.13.0 (branch) | **0.13.0** (crates.io, 2026-10-27) | release do mesmo branch bevy-0.20 |
| `landmass_rerecast` 0.3.0 | **0.4.0** (crates.io, 2026-10-27) | ponte para o rerecast 0.5 |

O `rerecast` **0.5.0** também resolve do registry, mas os dois fixes de
robustez do Viber não entraram nessa versão. Continua a ser o **único** path
vendor: `vendor/rerecast` é o 0.5.0 publicado + os dois fixes de
`src/poly_mesh.rs` (`VIBER_PATCH.md`), ligado por `[patch.crates-io]`.

Validação: `cargo check --lib` e `cargo test --test nav_navmesh` (7/7) +
`cargo test --lib nav::` (4/4) no Viber; `cargo check` do crate `vael` no HEAD
contra a engine migrada. O lock não tem dependências git.

**Atualização 2026-11-07**: os pins `bevy_* = "0.20.0-rc.2"` dos 5 forks
passaram a **`"0.20.1"`** (38 linhas). O requisito pré-release já resolvia
para 0.20.x no lockfile, mas dizia uma versão que já não é a que testamos —
com o `bevy` do engine e do editor em `0.20.1` (docs/CRATES.md, ronda de
2026-11-07) o pin acompanha. Nenhuma alteração de código nos forks.

**Atualização 2026-10-18**: o fork `bevy_rapier3d` foi **REMOVIDO** — o
upstream publicou o **0.37.0** (crates.io, 2026-10-10), que é exatamente a
versão oficial bevy-0.20: `bevy ^0.20`, `rapier3d 0.36.1`, `nalgebra 0.35`
(`convert-glam033`) — o port que o fork fazia à mão (soft bodies, `bevy_shape`
→ `bevy::math::bounding`, `.context()`) já lá está. O `bevy_rapier3d` resolve
agora do registry (features `dim3`/`async-collider`/`debug-render-3d`,
`default-features = false` mantidas). Compila e a suíte passa sem alterações
de API no nosso código.

**Atualização 2026-10-09**: o Bevy **0.20.0 final** saiu no crates.io
(2026-10-08); a tabela `[patch.crates-io]` de tags git foi **removida** e o
`bevy = "0.20"` resolve agora do registry.

Histórico: os forks foram criados quando o Bevy 0.20.0-rc.2 (tag GitHub
2026-09-25) ainda não estava no crates.io e os crates do ecossistema estavam
presos ao Bevy 0.19.

## Forks na altura em que foram criados

| Fork | Proveniência | Upstream | Estado no dia do fork |
|------|--------------|----------|----------------------|
| ~~`bevy_rapier3d` 0.36.0~~ **REMOVIDO 2026-10-18** | crates.io 0.36.0 (2026-08-08); repo `dimforge/bevy_rapier` **arquivado** 2026-09-24 — código vive agora em `dimforge/rapier` `bindings/bevy_rapier` | https://github.com/dimforge/rapier | substituído pelo **0.37.0** oficial (bevy ^0.20, rapier3d 0.36.1) do registry |
| ~~`bevy_rerecast` 0.5.0 + `bevy_rerecast_core` 0.5.0~~ **REMOVIDO 2026-11-11** | crates.io 0.5.0 (2026-08-05) | https://github.com/janhohenheim/rerecast | main ainda bevy 0.19; o 0.6.0 saiu a 2026-10-26 |
| ~~`bevy_landmass` 0.13.0~~ **REMOVIDO 2026-11-11** | git `andriyDev/landmass` branch `bevy-0.20`, rev `945b31cf1e7249e7bd24b08ad3f4cd7737916767` (2026-09) | https://github.com/andriyDev/landmass | port bevy-0.20-rc.1; o release 0.13.0 saiu a 2026-10-27 |
| ~~`landmass_rerecast` 0.3.0~~ **REMOVIDO 2026-11-11** | mesmo repo/rev do `bevy_landmass` | idem | ainda bevy ^0.19 no branch (WIP); o 0.4.0 saiu a 2026-10-27 |
| `rerecast` 0.4.0 → **0.5.0** | crates.io 0.4.0, rebase para 0.5.0 (2026-10-26) | https://github.com/janhohenheim/rerecast | bevy-independente. Os 2 fixes de robustez da triangulação (`VIBER_PATCH.md`) continuam fora do upstream |

## Alterações locais do fork rerecast (0.5.0)

* `bevy_reflect` 0.20.0 → **0.20.1** e glam **0.33.2** (a versão do
  `bevy_math` 0.20.1; o `^0.33` upstream resolveria igual);
* `tests/` e dev-dependencies removidas — nunca se constrói os testes do
  fork e eram elas que reancoravam o lock;
* os dois fixes de `src/poly_mesh.rs` (ver `VIBER_PATCH.md`).

Notas históricas dos forks já removidos (mantidas para referência):

* cross-deps entre forks por `path` (landmass_rerecast → bevy_landmass /
  bevy_rerecast; bevy_rerecast → bevy_rerecast_core);
* `bevy_rerecast`: removido o optional `bevy_rerecast_editor_integration`
  (arrastava UI de editor + bevy 0.19 para o Cargo.lock mesmo sem a feature) —
  no registry resolve-se com `default-features = false`;
* dev-dependencies e alvos `[[example]]/[[test]]/[[bench]]` removidos — nunca
  se constrói os testes deles e os dev-deps (egui/inspector…) é que traziam
  bevy 0.19 de volta à resolução;
* `bevy_landmass`: `landmass` (core) volta a vir do crates.io (é
  bevy-independente) — o release 0.13.0 mantém `landmass` 0.9 do registry;
* `bevy_rapier3d`: **rapier3d `=0.35.0-glamx0.2` → `0.36.0`** e nalgebra
  `0.34` (`convert-glam032`) → `0.35` (`convert-glam033`). O rapier 0.35 usa
  glam 0.32 (via `glamx` 0.2) e o bevy_rapier conta com o `Vec3` do rapier ser
  o MESMO tipo do Bevy — com o Bevy 0.20 em glam 0.33 isso dava ~200 erros de
  tipo. O rapier 0.36 (`glamx` 0.3 → glam 0.33) traz soft bodies: um
  `SoftBodySet` vazio em `RapierContextSimulation::soft_bodies` para as
  assinaturas novas de `step`/`remove`/`render`, `RigidBodyType::SoftFrame`
  mapeado para `RigidBody::Dynamic`, `ContactPair::manifolds()` (método),
  `handle_soft_body_tear_event` no-op, e `IntegrationParameters::soft_bodies`
  espelhado com `#[reflect(ignore)]`;
* `bevy_rerecast_core` / `bevy_rapier3d`: `bevy::math::bounding` → crate
  `bevy_shape` (raiz: `bevy_shape::Aabb3d`) — dependência `bevy_shape`
  acrescentada e redirecionada no `[patch.crates-io]`; `.context()` do anyhow
  pelo caminho completo (o prelude do bevy_ecs 0.20 traz `ContextExt`).
