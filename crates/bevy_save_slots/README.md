# bevy_save_slots

Save envelope, slots and atomic JSON I/O for Bevy games.

This crate owns the on-disk format only: the opaque `SaveGame` envelope,
slot paths, `world_kv`/`game` sections, terrain patches and the atomic
write/load. It has no plugin and no runtime state; the game decides when to
capture and what to put in `world_kv`/`game`.

```toml
[dependencies]
bevy_save_slots = "0.1"
```

```rust
use bevy_save_slots::{capture, load_from_disk, save_to_disk, save_path_for_slot};

let mut game = capture([12.0, 25.0, -8.0], (0.8, 0.5, 0.9));
game.scene = Some("main".into());
save_to_disk(&save_path_for_slot(Some("my-world"), None, Some(1)), &game)?;
let loaded = load_from_disk(&save_path_for_slot(Some("my-world"), None, Some(1)))?;
```

## What it contains

- `SaveGame` — generic envelope: pose, volumes, scene, came-from, locale,
  terrain patches, `world_kv` (flat JSON) and an opaque `game` section.
- `WorldSaveKey` / `SaveDir` — resources the runtime fills from the world
  path and `config.yaml`.
- `world_save_key`, `save_path_for`, `save_path_for_slot`, `list_save_slots`.
- `capture`, `save_to_disk` (tmp+fsync+rename), `load_from_disk` (with legacy
  migration).

The historical `viber::save::…` reflection paths are preserved with
`#[type_path]`, so the BRP/`viber.ecs` contract does not change.

## Bevy version

Built against Bevy 0.20.1 with `default-features = false` (ECS/reflect only).

## License

MIT OR Apache-2.0.
