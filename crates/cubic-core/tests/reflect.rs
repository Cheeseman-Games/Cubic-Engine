//! End-to-end reflection through the `#[derive(Component)]` /
//! `#[derive(Inspectable)]` macros, exactly as a game crate uses them.
//!
//! The rest of the reflect module is unit-tested with hand-written impls; this
//! file proves the derives produce everything those tests assume, so it must
//! stay gated on the same feature as the module it exercises.

#![cfg(feature = "reflect")]

use cubic_core::prelude::*;

/// Every kind a descriptor can draw a widget for, on one component.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mood {
    Idle,
    Warm,
    Angry,
}

impl Selectable for Mood {
    fn index(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Warm => 1,
            Self::Angry => 2,
        }
    }

    fn from_index(index: usize) -> Self {
        [Self::Idle, Self::Warm, Self::Angry][index]
    }
}

#[derive(Component, Inspectable)]
struct Character {
    name: String,
    health: f32,
    speed: i32,
    alive: bool,
    forward: Vec2,
    tint: Rgba,
    sprite: TextureHandle,
    font: FontHandle,
    // An unknown type is not editable through reflection, so without the
    // annotation this field would silently disappear.
    #[inspect(select = ["Idle", "Warm", "Angry"])]
    mood: Mood,
    // Annotate to skip a field reflection cannot describe.
    #[inspect(skip)]
    #[allow(dead_code)]
    secret: u64,
}

fn character() -> Character {
    Character {
        name: "Friendo".to_owned(),
        health: 80.0,
        speed: 4,
        alive: true,
        forward: Vec2::new(1.0, 0.0),
        tint: Rgba::rgb(1.0, 0.5, 0.0),
        sprite: TextureHandle::new(2u64.try_into().unwrap()),
        font: FontHandle::new(3u64.try_into().unwrap()),
        mood: Mood::Warm,
        secret: 4096,
    }
}

/// A component whose every field reflection cannot describe: it contributes its
/// name but no fields, so an inspector renders an empty, non-crashing panel.
#[derive(Component, Inspectable)]
struct Opaque {
    #[inspect(skip)]
    #[allow(dead_code)]
    blob: [u8; 64],
}

fn registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    registry
        .register::<Character>()
        .register::<Opaque>()
        .register::<Transform>();
    registry
}

#[test]
fn a_derived_component_registers_with_its_rust_type_name() {
    let registry = registry();
    let names: Vec<&str> = registry.names().collect();
    assert_eq!(names, ["Character", "Opaque", "Transform"]);

    let entry = registry.get("Character").unwrap();
    assert_eq!(entry.name, "Character");
    assert_eq!(entry.type_id, std::any::TypeId::of::<Character>());
}

/// The derived descriptors carry the kind each field's type implied.
#[test]
fn the_derive_describes_every_inferable_kind() {
    let registry = registry();
    let entry = registry.get("Character").unwrap();
    let kinds: Vec<FieldKind> = entry.fields.iter().map(Field::kind).collect();
    assert_eq!(
        kinds,
        [
            FieldKind::String,
            FieldKind::F32,
            FieldKind::I32,
            FieldKind::Bool,
            FieldKind::Vec2,
            FieldKind::Color,
            FieldKind::Asset {
                kind: cubic_core::assets::AssetKind::Texture
            },
            FieldKind::Asset {
                kind: cubic_core::assets::AssetKind::Font
            },
            FieldKind::Select(&["Idle", "Warm", "Angry"]),
        ],
        "the skipped field and the unknown-type field stay out"
    );
    let names: Vec<&str> = entry.fields.iter().map(Field::name).collect();
    assert_eq!(
        names,
        [
            "name", "health", "speed", "alive", "forward", "tint", "sprite", "font", "mood"
        ]
    );
}

/// `From<AssetHandle>` chains mean a typed handle's descriptor speaks the raw
/// handle by id, not the typed face.
#[test]
fn a_typed_asset_handle_reads_and_writes_as_a_raw_handle() {
    let registry = registry();
    let entry = registry.get("Character").unwrap();
    let sprite = entry.field("sprite").unwrap();
    assert_eq!(
        sprite.get(&character()),
        FieldValue::Asset(TextureHandle::new(2u64.try_into().unwrap()).asset())
    );

    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, character());
    let sprite_value = FieldValue::Asset(TextureHandle::new(7u64.try_into().unwrap()).asset());
    entry
        .set_field(&mut world, entity, sprite, &sprite_value)
        .expect("the sprite's descriptor accepts raw handle values");
    assert_eq!(
        world.get::<Character>(entity).unwrap().sprite,
        TextureHandle::new(7u64.try_into().unwrap())
    );
}

#[test]
fn an_enum_field_is_an_index_over_its_annotated_options() {
    let registry = registry();
    let entry = registry.get("Character").unwrap();
    let mood = entry.field("mood").unwrap();
    let component = character();

    assert_eq!(
        mood.get(&component),
        FieldValue::Select(1),
        "Warm is option 1"
    );

    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, character());

    entry
        .set_field(&mut world, entity, mood, &FieldValue::Select(2))
        .expect("a known option applies");
    assert_eq!(world.get::<Character>(entity).unwrap().mood, Mood::Angry);

    let error = entry
        .set_field(&mut world, entity, mood, &FieldValue::Select(9))
        .unwrap_err();
    assert_eq!(error, FieldError::SelectOutOfRange(9));
    assert_eq!(
        world.get::<Character>(entity).unwrap().mood,
        Mood::Angry,
        "an out-of-range option changes nothing"
    );
}

#[test]
fn field_setting_refuses_the_wrong_kind_of_value() {
    let registry = registry();
    let entry = registry.get("Character").unwrap();
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, character());
    let health = entry.field("health").unwrap();

    let error = entry
        .set_field(
            &mut world,
            entity,
            health,
            &FieldValue::String("heal".to_owned()),
        )
        .unwrap_err();
    assert_eq!(
        error,
        FieldError::WrongKind {
            expected: "f32",
            found: "string"
        }
    );
    assert_eq!(world.get::<Character>(entity).unwrap().health, 80.0);
}

/// The registry shows an entity the components it actually holds, where the
/// inspector reads its selections from.
#[test]
fn on_entity_lists_what_an_entity_holds_by_registered_name() {
    let registry = registry();
    let mut world = World::new();
    let hero = world.spawn();
    world.insert(hero, character());
    world.insert(hero, Transform::default());
    let bare = world.spawn();

    let hero_names: Vec<&str> = registry
        .on_entity(&world, hero)
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(hero_names, ["Character", "Transform"]);
    assert!(registry.on_entity(&world, bare).is_empty());
}

/// A component reflection cannot describe still names itself in a listing.
#[test]
fn an_opaque_component_lists_but_has_no_fields() {
    let registry = registry();
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Opaque { blob: [0u8; 64] });

    let entry = registry.get("Opaque").unwrap();
    assert!(entry.fields.is_empty());

    let listed: Vec<&str> = registry
        .on_entity(&world, entity)
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(listed, ["Opaque"]);
}

#[test]
fn engine_defaults_carries_transform_with_its_three_fields() {
    let registry = ComponentRegistry::engine_defaults();
    let transform = registry.get("Transform").unwrap();
    assert_eq!(
        transform.fields.iter().map(Field::kind).collect::<Vec<_>>(),
        [FieldKind::Vec2, FieldKind::F32, FieldKind::Vec2]
    );
    assert_eq!(
        transform.fields.iter().map(Field::name).collect::<Vec<_>>(),
        ["position", "rotation", "scale"]
    );

    let mut world = World::new();
    let entity = world.spawn();
    let expected = Transform {
        position: Vec2::new(3.0, 4.0),
        ..Transform::default()
    };
    world.insert(entity, expected);

    assert_eq!(
        transform
            .field("position")
            .unwrap()
            .get(transform.read(&world, entity).unwrap()),
        FieldValue::Vec2(Vec2::new(3.0, 4.0))
    );
}

#[test]
fn derived_field_values_render_for_the_inspector() {
    let registry = registry();
    let entry = registry.get("Character").unwrap();
    let display: Vec<String> = entry
        .fields
        .iter()
        .map(|field| field.get(&character()).to_string())
        .collect();
    assert_eq!(
        display,
        [
            "Friendo",
            "80",
            "4",
            "true",
            "(1, 0)",
            "(1, 0.5, 0, 1)",
            "asset#2",
            "asset#3",
            "#1"
        ],
        "a typed handle's descriptor prints the raw handle it stores"
    );
}
