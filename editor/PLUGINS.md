# Editor plugins

Project plugins live in `<project root>/editor/plugins/*.lua` (or `.luau`). The
editor loads them at startup, calls them about 10×/s and reloads a changed file
within ~1 s. A plugin is a Luau chunk that returns a **manifest table**; the
editor only knows generic capabilities (scene, selection, files, UI), never a
game rule.

A plugin file that only returns `{ on_update = function(ctx) ... end }` keeps
working: every callback except `render` is optional.

## Manifest

```lua
return {
  api = 1,                       -- optional; the host understands 1
  id = "quests",                 -- optional; defaults to the file stem
  title = "Missões",             -- optional; defaults to `id`

  on_start = function(ctx) end,  -- once after load / after a hot reload
  on_update = function(ctx) end, -- ~10 Hz
  on_event = function(ctx, event) end,
  on_command = function(ctx, id) end,
  on_stop = function(ctx) end,   -- reserved; hot reload currently drops state

  panels = {
    { id = "quests", title = "Missões", render = function(ctx) return ui end },
  },
  commands = {
    { id = "new", title = "Quests: nova", keywords = "criar missão" },
  },
  nodes = {
    { tag = "ResourceNode", label = "…", snippet = '<ResourceNode />', group = "content" },
  },
  lint = function(ctx) return { { severity = "warning", kind = "quest-ref", message = "…" } } end,
}
```

Rules:

- `id`, panel ids and command ids accept letters, digits, `_`, `-` and `.`.
- Panel/command ids are unique **inside** the plugin; the host namespaces them
  as `plugin:panel` / `plugin:command`.
- A panel needs `render(ctx)`. `render` must be pure: return a tree, do not call
  `editor.*` actions from it (read-only `editor.fs`/`editor.node` is fine).
- A snippet in `nodes` must parse; it is offered in **Add Node** and overrides
  an engine catalogue entry with the same tag. The engine catalogue itself is
  unchanged.

## `ctx`

| Field | Meaning |
|---|---|
| `dt` | Nominal seconds since the previous tick (`0.1`). |
| `time` | Monotonic seconds since the editor started. |
| `tick` | Monotonic tick counter. |
| `nodes` | Only nodes with `groups=`: `selector`, `name`, `tag`, `groups` (set), `props` (resolved `--@export`). |
| `player` | `{x,y,z}` or `nil` (edit mode may not have spawned the hero yet). |
| `selection` | Selector of the primary editor selection, or `nil`. |
| `camera` | `{target={x,y,z}, distance, pitch, yaw}` or `nil`. |
| `project` | `{root, name, main_scene, scenes_dir, scripts_dir, assets_roots}`. Paths are project-relative with `/`. |
| `doc` | `{scene, revision, dirty}`. |

### `ctx.editor`

Read-only queries:

- `node(selector)` → `{selector, name, tag, script, attrs, props}` or `nil`.
- `find_nodes{name=, tag=, script=, attr={key=value}, limit=}` → array. `limit`
  defaults to 200 (max 1000). `tag` is case-insensitive; the other fields are
  exact matches.

Selection:

- `select(selector)`, `select_many{selector1, selector2, …}`.

Undoable scene edits (queued and applied after all callbacks, one undo step):

- `set_attr(selector, key, value)` — `nil` removes the attribute.
- `set_props(selector, {key=value, …})` — adds the `prop-` prefix; `nil` removes.
- `insert(parent, xml, index?)`, `remove(selector)`, `move(selector, parent, index?)`,
  `duplicate(selector)`, `extract(selector, to?)`, `create_file(path, xml, title?)`.

Editor commands:

- `save()`, `open_scene(path)`, `open_panel(panel_id)`.
- `toast(kind, message)` — `info`, `success`, `warn`, `error`.
- `log(message)`, `warn(message)`, `error(message)`.

Presentation (the old API, unchanged):

- `attach{slot, url, bone, x, y, z, rx, ry, rz, scale}` — radians, like
  `viber.attach`; the host re-requests only when the values change and waits for
  the bone.
- `detach(slot)`.
- `focus_player{distance, height, pitch, yaw}` → `true` when the hero exists.

Files (jailed to the project root):

- `fs.read(path)` → `text, err`
- `fs.write(path, text)` → `ok, err` (atomic; keeps a `.bak`; creates missing
  parent folders inside the project root)
- `fs.list(path)` → `{ {name, path, dir}, … }, err`
- `fs.exists(path)` → boolean
- `fs.mkdir(path)` → `ok, err`
- `fs.json.decode(text)` → `value, err`
- `fs.json.encode(value, pretty?)` → `text, err`
- `fs.json.array(values)` — marks a table as a JSON array. Use it for empty
  arrays: a plain empty Lua table encodes as `{}`.

Paths are project-relative. Absolute paths, `..`, and symlinks that escape the
root are rejected. Writing an open scene file, `config.yaml`, `world.data/` or
`assets/` is rejected; use the scene operations instead.

Preferences:

- `prefs.get(key)` → value or `nil`
- `prefs.set(key, value)` → `ok, err`

Values are JSON-compatible (nil removes the key). Prefs are namespaced by plugin
and saved in `~/.config/viber/editor.json` (or `$XDG_CONFIG_HOME`).

## UI tree

`render(ctx)` returns a table. Supported `type`s:

| Type | Fields |
|---|---|
| `column` / `row` | `gap`, `children` |
| `heading` | `text` |
| `label` | `text`, `tone` = `dim`/`warn`/`error` |
| `button` | `id`, `text`, `primary`, `enabled`, `tip` |
| `text` | `id`, `label`, `value`, `placeholder` |
| `number` | `id`, `label`, `value`, `step`, `min`, `max` |
| `checkbox` | `id`, `label`, `value` |
| `select` | `id`, `label`, `value`, `options = {{value,label}, …}` |
| `list` | `id`, `selected`, `items = {{id,label,status}, …}`, `height` |
| `text_lines` | `id`, `label`, `values = {…}` |
| `separator` | — |
| `scroll` | `height`, `children` |
| `spacer` | `size` |

Limits: depth 32, 5 000 nodes, 64 KB per text, unique interactive ids.

### Events

`on_event(ctx, event)` receives:

```lua
{ kind = "click" | "change" | "select" | "add" | "remove",
  panel = "panel-id", id = "widget-id",
  value = "…" | nil, index = 0-based | nil }
```

- `click` — button.
- `change` — text (every changed value), number (on commit), checkbox
  (`"true"`/`"false"`), select (option value), a text line (`index`).
- `select` — list row (`value` is the row id).
- `add` / `remove` — `text_lines` (remove has `index`).

Text inputs are polled; the host delays a tree rebuild while one is focused, so
the cursor is not lost. Events are delivered on the next 10 Hz tick.

## Commands and lint

- `commands` appear in Ctrl+K as `title — plugin`; `on_command(ctx, id)` runs
  immediately and its queued actions are applied on the next tick.
- `lint(ctx)` returns `{ {severity="error"|"warning", kind="…", message="…"}, … }`.
  The Problems dock merges them with the engine lint; the tab badge counts both.
  Lint runs on save/revalidate, synchronously.

## Hot reload and errors

- The plugin folder is checked once per second. A changed file is compiled and
  swapped atomically; if it fails, the previous working version stays loaded and
  the error is shown in the Plugins tab.
- Module-level Lua state is reset by the swap. Persistent data belongs in
  `editor.prefs`; content belongs in the project files.
- A broken plugin never stops the others. Errors are logged once per unique
  message.

## Smoke tests

The headless tests can load a real project without opening a window:

```bash
PLUGIN_SMOKE_DIR=/path/to/project/editor/plugins \
  cargo test -p vibereditor plugins::tests:: -- --nocapture
```

They compile/execute the manifests, run every `lint`, `on_start`/`on_update`
and `render` against `world.xml` (when present), and load
`scripts/lib/rules.lua` and `scripts/lib/landmarks.lua` with a small `viber`
stub. Without `PLUGIN_SMOKE_DIR` the tests skip.

## Security

The Lua VM uses `StdLib::ALL_SAFE` with a 64 MB limit; `require`, `io`, `os` and
`package` are not available. The only project access is `editor.fs`, jailed as
described above. Plugins are project code, but the host still validates every
shape (manifest, UI tree, edits, paths) so a mistake becomes an error card, not a
corrupted world.
