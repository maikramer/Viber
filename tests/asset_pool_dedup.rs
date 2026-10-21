//! Guarda da política de assets depois do split.
//!
//! O pool do RPG vive no repo irmão Vael (`../Vael/assets/public`). Neste
//! checkout o único conjunto de binários é `examples/racer-assets`: o subset
//! que a pista do simple-racer cita. Uma pasta `assets/` dentro de um exemplo
//! volta a ser um espelho e o teste parte. `worlds/assets` continua banido.

use std::path::Path;

/// GLBs que a pista referencia. Um ficheiro a mais no subset é regressão
/// (o resto do catálogo fica no Vael).
const RACER_MESHES: &[&str] = &[
    "forest/dead_tree_lod0.glb",
    "forest/dead_tree_lod1.glb",
    "forest/dead_tree_lod2.glb",
    "forest/pine_dark_lod0.glb",
    "forest/pine_dark_lod1.glb",
    "forest/pine_dark_lod2.glb",
    "forest/tree_oak_lod0.glb",
    "forest/tree_oak_lod1.glb",
    "forest/tree_oak_lod2.glb",
    "forest/tree_pine_lod0.glb",
    "forest/tree_pine_lod1.glb",
    "forest/tree_pine_lod2.glb",
    "infra/city_gate_arch_lod0.glb",
    "props/rock_mossy_lod0.glb",
    "props/rock_mossy_lod1.glb",
    "props/rock_mossy_lod2.glb",
    "vehicles/kart_hero_body_lod0.glb",
    "vehicles/kart_rival_body_lod0.glb",
    "vehicles/kart_wheel_lod0.glb",
    "village/iron_brazier_lod0.glb",
    "village/market_stall_lod0.glb",
    "village/village_house_lod0.glb",
    "village/village_longhouse_lod0.glb",
    "village/watchtower_lod0.glb",
    "village/wooden_barrel_lod0.glb",
    "village/wooden_crate_lod0.glb",
];

#[test]
fn racer_mesh_subset_stays_on_the_allowlist() {
    let meshes =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/racer-assets/public/assets/meshes");
    if !meshes.is_dir() {
        eprintln!("skip: subset do racer sem binários neste checkout");
        return;
    }
    let mut extras = Vec::new();
    collect(&meshes, &meshes, &mut extras);
    assert!(
        extras.is_empty(),
        "meshes fora do subset da pista — o catálogo RPG vive no Vael:\n  {}",
        extras.join("\n  ")
    );
}

#[test]
fn examples_do_not_carry_a_local_assets_tree() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&examples)
        .expect("examples dir")
        .flatten()
    {
        if entry.file_name().to_string_lossy() == "racer-assets" {
            continue;
        }
        let assets = entry.path().join("assets");
        if assets.exists() {
            found.push(assets.display().to_string());
        }
    }
    assert!(
        found.is_empty(),
        "exemplo com pasta assets/ própria — usa racer-assets ou o Vael:\n  {}",
        found.join("\n  ")
    );
}

/// `worlds/assets` já foi um symlink para o pool — banido.
#[test]
fn worlds_assets_symlink_stays_dead() {
    let worlds_assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("worlds/assets");
    let meta = std::fs::symlink_metadata(&worlds_assets);
    assert!(
        meta.is_err(),
        "worlds/assets voltou a existir ({:?})",
        meta.map(|m| m.file_type())
    );
}

fn collect(root: &Path, dir: &Path, extras: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, extras);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !RACER_MESHES.contains(&rel.as_str()) {
                extras.push(rel);
            }
        }
    }
}
