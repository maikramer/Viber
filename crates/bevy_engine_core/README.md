# bevy_engine_core

Shared contracts for Viber's `bevy_*` engine packages. This is the bottom of
the dependency graph: markers, schedule sets, shared resources and the
deterministic RNG. No renderer, no window, no game types.

```toml
[dependencies]
bevy_engine_core = "0.1"
```

```rust
use bevy_engine_core::{MenusOpen, PlayerBody, UiSet};
```

## What it contains

- `PlayerBody` — marker for the entity owned by a player controller.
- `MenusOpen` — modal/shop/travel/map/cinematic input gate.
- `ScriptToast` — toast message shared by runtime and save layers.
- `AudioMixerSettings` — master/music/sfx/voice/ambience bus volumes.
- `UiSet` — the declarative UI schedule order.
- `TerrainQuery` / `NavQuery` — traits for services that must not depend on
  the terrain/navigation crates.
- `rng::Rng` — SplitMix64, the deterministic RNG behind "same seed, same
  world".
- `script_kind` — script path → creature kind and kill XP.

## Bevy version

Built against Bevy 0.20.1. The crate depends on `bevy` with
`default-features = false`, so it only pulls ECS/reflect/math.

## License

MIT OR Apache-2.0.
