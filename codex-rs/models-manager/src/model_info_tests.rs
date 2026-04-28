use super::*;
use crate::ModelsManagerConfig;
use codex_protocol::config_types::Personality;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(true),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn model_context_window_override_clamps_to_max_context_window() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig {
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.context_window = Some(400_000);

    assert_eq!(updated, expected);
}

#[test]
fn model_context_window_uses_model_value_without_override() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig::default();

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn base_instructions_override_keeps_personality_messages_when_enabled() {
    let model = model_info_from_slug("gpt-5.2-codex");
    let config = ModelsManagerConfig {
        base_instructions: Some("override instructions".to_string()),
        personality_enabled: true,
        ..Default::default()
    };

    let updated = with_config_overrides(model, &config);

    assert_eq!(updated.base_instructions, "override instructions");
    assert_eq!(
        updated.get_model_instructions(Some(Personality::Friendly)),
        "override instructions"
    );
    assert_eq!(
        updated
            .model_messages
            .as_ref()
            .and_then(|messages| messages.get_personality_message(Some(Personality::Friendly))),
        Some(LOCAL_FRIENDLY_TEMPLATE.to_string())
    );
}

#[test]
fn known_local_personality_slug_restores_messages_when_remote_match_has_none() {
    let mut model = model_info_from_slug("gpt-5.2");
    model.slug = "gpt-5.2-codex".to_string();
    model.model_messages = None;
    let config = ModelsManagerConfig {
        personality_enabled: true,
        ..Default::default()
    };

    let updated = with_config_overrides(model, &config);

    assert_eq!(
        updated
            .model_messages
            .as_ref()
            .and_then(|messages| messages.get_personality_message(Some(Personality::Friendly))),
        Some(LOCAL_FRIENDLY_TEMPLATE.to_string())
    );
}

#[test]
fn base_instructions_override_drops_personality_messages_when_disabled() {
    let model = model_info_from_slug("gpt-5.2-codex");
    let config = ModelsManagerConfig {
        base_instructions: Some("override instructions".to_string()),
        personality_enabled: false,
        ..Default::default()
    };

    let updated = with_config_overrides(model, &config);

    assert_eq!(updated.base_instructions, "override instructions");
    assert_eq!(updated.model_messages, None);
}
