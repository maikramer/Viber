//! Round-trip XML → `.bsn` → `XmlNode` e overrides da camada de editor.

use std::fs;
use std::path::Path;

use bevy_world_xml::bsn::{
    EmitOptions, emit_file, load_bsn_world, load_bsn_world_in, worlds_equivalent,
};
use bevy_world_xml::xml::include::load_world;

fn roundtrip(dir: &Path, xml: &Path, bsn: &Path) {
    let text = emit_file(xml, dir, EmitOptions::default()).expect("emitir bsn");
    fs::write(bsn, text).expect("gravar bsn");

    let from_xml = load_world(xml).expect("xml");
    let from_bsn = load_world(bsn).expect("bsn");
    worlds_equivalent(
        &from_xml.root_attrs,
        &from_xml.nodes,
        &from_bsn.root_attrs,
        &from_bsn.nodes,
    )
    .unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn xml_vira_bsn_e_volta_igual() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r##"<world static="true" title="Teste">
  <Entity name="a" translation="1 2 3" euler="0 90 0">
    <GltfScene url="/assets/x.glb" lod1-url="/assets/x_lod1.glb" />
    <PointLight color="#ff0000" intensity="500" shadows="1" />
  </Entity>
  <Group name="g">
    <Entity name="b" transform="pos: 4 5 6; euler: 10 20 30; scale: 2" />
  </Group>
</world>
"##,
    )
    .expect("xml");
    let bsn = dir.path().join("world.bsn");
    roundtrip(dir.path(), &xml, &bsn);
}

#[test]
fn includes_em_bsn_expandem_como_xml() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("part.xml"),
        r#"<world><Entity name="child"><GltfScene url="/assets/c.glb" /></Entity></world>"#,
    )
    .expect("part");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true">
  <Entity name="root">
    <Include src="part.xml" translation="0 0 5" />
  </Entity>
</world>
"#,
    )
    .expect("world");
    let part_bsn = dir.path().join("part.bsn");
    fs::write(
        &part_bsn,
        emit_file(
            &dir.path().join("part.xml"),
            dir.path(),
            EmitOptions::default(),
        )
        .expect("emit part"),
    )
    .expect("part bsn");
    let bsn = dir.path().join("world.bsn");
    roundtrip(dir.path(), &xml, &bsn);
}

#[test]
fn transform_e_name_do_editor_mandam_nos_attrs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><Entity name="a" translation="1 2 3"><PointLight intensity="500" /></Entity></world>"#,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let text = text.replace("x: 1.0, y: 2.0, z: 3.0", "x: 10.0, y: 20.0, z: 30.0");
    let text = text.replace("#\"a\"", "#\"renamed\"");
    let text = text.replace("intensity: 500.0", "intensity: 900.0");
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");

    let world = load_bsn_world(&bsn).expect("load");
    let node = &world.nodes[0];
    assert_eq!(node.tag, "Entity");
    assert_eq!(node.attr("name"), Some("renamed"));
    assert_eq!(node.attr("translation"), Some("10 20 30"));
    let light = &node.children[0];
    assert_eq!(light.tag, "PointLight");
    assert_eq!(light.attr("intensity"), Some("900"));
}

#[test]
fn gltf_source_do_editor_muda_o_url() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><GltfScene name="m" url="/assets/old.glb" /></world>"#,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let text = text.replace("path: \"assets/old.glb\"", "path: \"assets/new.glb\"");
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");

    let world = load_bsn_world(&bsn).expect("load");
    assert_eq!(world.nodes[0].attr("url"), Some("/assets/new.glb"));
}

#[test]
fn include_em_memoria_nao_depende_de_ficheiros_bsn() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("part.xml"),
        r#"<world><Entity name="child"><GltfScene url="/assets/c.glb" /></Entity></world>"#,
    )
    .expect("part");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><Include src="part.xml" /></world>"#,
    )
    .expect("world");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");

    let world = load_bsn_world_in(&bsn, dir.path()).expect("load");
    assert_eq!(world.nodes.len(), 1);
    assert_eq!(world.nodes[0].tag, "Entity");
    assert_eq!(world.nodes[0].attr("name"), Some("child"));
}
