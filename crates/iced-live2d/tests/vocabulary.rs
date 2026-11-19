//! What `#[derive(Vocabulary)]` writes, exercised through the public API.
//!
//! Runs only with the `macros` feature: `cargo test -p iced-live2d --features macros`.

#![cfg(feature = "macros")]

use std::sync::Arc;

use iced_live2d::{AssetId, KeyId, Vocabulary, VocabularySpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(prefix = "Param")]
enum Face {
    AngleX,
    #[name("ParamEyeBallX")]
    EyeBallX,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
enum Plain {
    Idle,
    TapBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
enum Empty {}

fn id(index: usize, name: &str) -> KeyId {
    KeyId {
        asset: AssetId::unique(),
        index,
        name: Arc::from(name),
    }
}

#[test]
fn the_prefix_and_the_name_override_decide_the_names() {
    assert_eq!(
        Face::SPEC,
        VocabularySpec::Static(&["ParamAngleX", "ParamEyeBallX"])
    );
    assert_eq!(Face::AngleX.name(), "ParamAngleX");
    assert_eq!(Face::EyeBallX.name(), "ParamEyeBallX");
}

#[test]
fn the_index_is_the_declaration_order() {
    assert_eq!(Face::AngleX.index(), 0);
    assert_eq!(Face::EyeBallX.index(), 1);
    assert_eq!(Face::ALL, &[Face::AngleX, Face::EyeBallX]);
}

#[test]
fn a_key_round_trips_through_the_render_layers_identity() {
    assert_eq!(Face::from_id(&id(1, "ParamEyeBallX")), Some(Face::EyeBallX));
    assert_eq!(Face::from_id(&id(9, "nowhere")), None);
}

#[test]
fn without_a_prefix_the_variant_name_is_the_model_name() {
    assert_eq!(Plain::SPEC, VocabularySpec::Static(&["Idle", "TapBody"]));
    assert_eq!(Plain::TapBody.index(), 1);
}

#[test]
fn an_empty_vocabulary_has_no_keys() {
    assert_eq!(Empty::SPEC, VocabularySpec::Static(&[]));
    assert!(Empty::ALL.is_empty());
    assert_eq!(Empty::from_id(&id(0, "anything")), None);
}

#[test]
fn a_derived_key_belongs_to_no_asset() {
    assert_eq!(Face::AngleX.asset(), None);
}
