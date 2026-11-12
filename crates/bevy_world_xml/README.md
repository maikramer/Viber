# bevy_world_xml

Declarative world formats for Bevy: XML, BSN and the `.vinst` binary.

This is the format layer: the element tree, the tolerant parser, include
expansion, project discovery, typed value parsers, the BSN world tree and the
compact vegetation instance format. The YAML `GameConfig` and the live scene
catalog stay in higher layers.

```toml
[dependencies]
bevy_world_xml = "0.1"
```

```rust
use bevy_world_xml::{XmlNode, XmlDocument, parse_file};
```

## What it contains

- `XmlNode` / `XmlDocument` — the element tree with raw string attributes,
  text and optional file origin.
- `parse_file`, `parse_fragment`, `normalize_bare_bools` — the tolerant
  world parser (bare boolean attributes like `<Fog enabled>`).
- `values` — typed parsers for bool/f32/vec/color attributes.
- `include` — recursive `<Include src="…">` expansion (depth, cycles,
  project-root paths and pose wrappers).
- `emit` — the owned tree back to well-formed XML (bake/copy/paste).
- `project` — `config.yaml` discovery, `root_of`, scenes catalog and entries.
- `bsn` — `WorldNode`/`WorldProp`, load/emit, pose helpers and world
  equivalence, built on `jackdaw_bsn` (pinned to the Bevy 0.20 line).
- `vinst::{encode, decode}` — `.vinst` (VINS, zlib) vegetation instances.
- `PlacedInstance` — one placed template instance.
- `DeltaPatch` — one writable terrain patch stored in the save.

## Bevy version

Built against Bevy 0.20.1 with `default-features = false` (reflect/math only).

## License

MIT OR Apache-2.0.
