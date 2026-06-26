use super::*;
use assert_matches::assert_matches;
use codex_config::types::ModelAvailabilityNuxConfig;
use codex_protocol::openai_models::ModelAvailabilityNux;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc::unbounded_channel;

fn all_model_presets() -> Vec<ModelPreset> {
    crate::test_support::TEST_MODEL_PRESETS.clone()
}

/// dsx's bundled deepseek presets ship without an `upgrade` configured, so the
/// migration-prompt tests synthesize one: `deepseek-v4-flash` upgrades to
/// `deepseek-v4-pro`.
fn migration_presets() -> Vec<ModelPreset> {
    let mut presets = all_model_presets();
    let flash = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-flash")
        .expect("deepseek-v4-flash preset present");
    flash.upgrade = Some(ModelUpgrade {
        id: "deepseek-v4-pro".to_string(),
        migration_config_key: HIDE_GPT5_1_MIGRATION_PROMPT_CONFIG.to_string(),
        model_link: None,
        upgrade_copy: None,
        migration_markdown: None,
    });
    presets
}

fn model_availability_nux_config(shown_count: &[(&str, u32)]) -> ModelAvailabilityNuxConfig {
    ModelAvailabilityNuxConfig {
        shown_count: shown_count
            .iter()
            .map(|(model, count)| ((*model).to_string(), *count))
            .collect(),
    }
}

fn model_migration_copy_to_plain_text(copy: &crate::model_migration::ModelMigrationCopy) -> String {
    if let Some(markdown) = copy.markdown.as_ref() {
        return markdown.clone();
    }
    let mut s = String::new();
    for span in &copy.heading {
        s.push_str(&span.content);
    }
    s.push('\n');
    s.push('\n');
    for line in &copy.content {
        for span in &line.spans {
            s.push_str(&span.content);
        }
        s.push('\n');
    }
    s
}

#[tokio::test]
async fn model_migration_prompt_only_shows_for_deprecated_models() {
    let seen = BTreeMap::new();
    let presets = migration_presets();
    // A model that carries an `upgrade` config shows the migration prompt.
    assert!(should_show_model_migration_prompt(
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        &seen,
        &presets
    ));
    // Any model migrating toward an upgrade target shows the prompt.
    assert!(should_show_model_migration_prompt(
        "legacy-model",
        "deepseek-v4-pro",
        &seen,
        &presets
    ));
    // Migrating to the same model never shows a prompt.
    assert!(!should_show_model_migration_prompt(
        "deepseek-v4-pro",
        "deepseek-v4-pro",
        &seen,
        &presets
    ));
}

#[test]
fn select_model_availability_nux_picks_only_eligible_model() {
    let mut presets = all_model_presets();
    presets.iter_mut().for_each(|preset| {
        preset.availability_nux = None;
    });
    let target = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-pro")
        .expect("target preset present");
    target.availability_nux = Some(ModelAvailabilityNux {
        message: "deepseek-v4-pro is available".to_string(),
    });

    let selected = select_model_availability_nux(&presets, &model_availability_nux_config(&[]));

    assert_eq!(
        selected,
        Some(StartupTooltipOverride {
            model_slug: "deepseek-v4-pro".to_string(),
            message: "deepseek-v4-pro is available".to_string(),
        })
    );
}

#[test]
fn select_model_availability_nux_skips_missing_and_exhausted_models() {
    let mut presets = all_model_presets();
    presets.iter_mut().for_each(|preset| {
        preset.availability_nux = None;
    });
    let pro = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-pro")
        .expect("deepseek-v4-pro preset present");
    pro.availability_nux = Some(ModelAvailabilityNux {
        message: "deepseek-v4-pro is available".to_string(),
    });
    let flash = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-flash")
        .expect("deepseek-v4-flash preset present");
    flash.availability_nux = Some(ModelAvailabilityNux {
        message: "deepseek-v4-flash is available".to_string(),
    });

    let selected = select_model_availability_nux(
        &presets,
        &model_availability_nux_config(&[(
            "deepseek-v4-pro",
            MODEL_AVAILABILITY_NUX_MAX_SHOW_COUNT,
        )]),
    );

    assert_eq!(
        selected,
        Some(StartupTooltipOverride {
            model_slug: "deepseek-v4-flash".to_string(),
            message: "deepseek-v4-flash is available".to_string(),
        })
    );
}

#[test]
fn select_model_availability_nux_uses_existing_model_order_as_priority() {
    let mut presets = all_model_presets();
    presets.iter_mut().for_each(|preset| {
        preset.availability_nux = None;
    });
    let first = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-flash")
        .expect("deepseek-v4-flash preset present");
    first.availability_nux = Some(ModelAvailabilityNux {
        message: "first".to_string(),
    });
    let second = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-pro")
        .expect("deepseek-v4-pro preset present");
    second.availability_nux = Some(ModelAvailabilityNux {
        message: "second".to_string(),
    });

    let selected = select_model_availability_nux(&presets, &model_availability_nux_config(&[]));

    // `deepseek-v4-pro` has priority 0 and therefore precedes
    // `deepseek-v4-flash` in catalog order, so it wins.
    assert_eq!(
        selected,
        Some(StartupTooltipOverride {
            model_slug: "deepseek-v4-pro".to_string(),
            message: "second".to_string(),
        })
    );
}

#[test]
fn select_model_availability_nux_returns_none_when_all_models_are_exhausted() {
    let mut presets = all_model_presets();
    presets.iter_mut().for_each(|preset| {
        preset.availability_nux = None;
    });
    let target = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-pro")
        .expect("target preset present");
    target.availability_nux = Some(ModelAvailabilityNux {
        message: "deepseek-v4-pro is available".to_string(),
    });

    let selected = select_model_availability_nux(
        &presets,
        &model_availability_nux_config(&[(
            "deepseek-v4-pro",
            MODEL_AVAILABILITY_NUX_MAX_SHOW_COUNT,
        )]),
    );

    assert_eq!(selected, None);
}

#[tokio::test]
async fn prepare_startup_tooltip_override_persists_model_availability_nux_count() {
    let codex_home = tempdir().expect("temp codex home");
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    let mut presets = all_model_presets();
    presets.iter_mut().for_each(|preset| {
        preset.availability_nux = None;
    });
    let target = presets
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-pro")
        .expect("target preset present");
    target.availability_nux = Some(ModelAvailabilityNux {
        message: "deepseek-v4-pro is available".to_string(),
    });

    let tooltip =
        prepare_startup_tooltip_override(&mut config, &presets, /*is_first_run*/ false).await;

    assert_eq!(tooltip.as_deref(), Some("deepseek-v4-pro is available"));
    assert_eq!(
        config.model_availability_nux.shown_count,
        HashMap::from([("deepseek-v4-pro".to_string(), 1)])
    );

    let reloaded = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("reloaded config");
    assert_eq!(
        reloaded.model_availability_nux.shown_count,
        HashMap::from([("deepseek-v4-pro".to_string(), 1)])
    );
}

#[tokio::test]
async fn accepted_model_migration_persists_target_default_reasoning_effort() {
    let codex_home = tempdir().expect("temp codex home");
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    config.model = Some("deepseek-v4-flash".to_string());
    config.model_reasoning_effort = Some(ReasoningEffortConfig::XHigh);

    let (tx_raw, mut rx) = unbounded_channel();
    let app_event_tx = AppEventSender::new(tx_raw);

    apply_accepted_model_migration(
        &mut config,
        &app_event_tx,
        "deepseek-v4-flash".to_string(),
        "deepseek-v4-pro".to_string(),
        ReasoningEffortConfig::Medium,
    );

    assert_eq!(config.model.as_deref(), Some("deepseek-v4-pro"));
    assert_eq!(
        config.model_reasoning_effort,
        Some(ReasoningEffortConfig::Medium)
    );

    let acknowledged = rx.try_recv().expect("acknowledged event");
    assert_matches!(
        acknowledged,
        AppEvent::PersistModelMigrationPromptAcknowledged { from_model, to_model }
            if from_model == "deepseek-v4-flash" && to_model == "deepseek-v4-pro"
    );

    let update_model = rx.try_recv().expect("update model event");
    assert_matches!(
        update_model,
        AppEvent::UpdateModel(model) if model == "deepseek-v4-pro"
    );

    let update_effort = rx.try_recv().expect("update effort event");
    assert_matches!(
        update_effort,
        AppEvent::UpdateReasoningEffort(Some(ReasoningEffortConfig::Medium))
    );

    let persist_selection = rx.try_recv().expect("persist model selection event");
    assert_matches!(
        persist_selection,
        AppEvent::PersistModelSelection { model, effort }
            if model == "deepseek-v4-pro" && effort == Some(ReasoningEffortConfig::Medium)
    );
}

#[tokio::test]
async fn model_migration_prompt_respects_hide_flag_and_self_target() {
    let mut seen = BTreeMap::new();
    seen.insert("deepseek-v4-flash".to_string(), "deepseek-v4-pro".to_string());
    assert!(!should_show_model_migration_prompt(
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        &seen,
        &migration_presets()
    ));
    assert!(!should_show_model_migration_prompt(
        "deepseek-v4-pro",
        "deepseek-v4-pro",
        &seen,
        &migration_presets()
    ));
}

#[tokio::test]
async fn model_migration_prompt_skips_when_target_missing_or_hidden() {
    // Upgrade target missing from the catalog -> no prompt.
    let mut available = all_model_presets();
    let mut current = available
        .iter()
        .find(|preset| preset.model == "deepseek-v4-flash")
        .cloned()
        .expect("preset present");
    current.upgrade = Some(ModelUpgrade {
        id: "missing-target".to_string(),
        migration_config_key: HIDE_GPT5_1_MIGRATION_PROMPT_CONFIG.to_string(),
        model_link: None,
        upgrade_copy: None,
        migration_markdown: None,
    });
    available.retain(|preset| preset.model != "deepseek-v4-flash");
    available.push(current.clone());

    assert!(!should_show_model_migration_prompt(
        &current.model,
        "missing-target",
        &BTreeMap::new(),
        &available,
    ));

    assert!(target_preset_for_upgrade(&available, "missing-target").is_none());

    // Upgrade target hidden from the picker -> no prompt (even though the
    // source model has an upgrade configured via `migration_presets`).
    let mut with_hidden_target = migration_presets();
    let target = with_hidden_target
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-pro")
        .expect("target preset present");
    target.show_in_picker = false;

    assert!(!should_show_model_migration_prompt(
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        &BTreeMap::new(),
        &with_hidden_target,
    ));
    assert!(target_preset_for_upgrade(&with_hidden_target, "deepseek-v4-pro").is_none());
}

#[tokio::test]
async fn model_migration_prompt_shows_for_hidden_model() {
    let codex_home = tempdir().expect("temp codex home");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config");

    let mut available_models = migration_presets();
    let current = available_models
        .iter_mut()
        .find(|preset| preset.model == "deepseek-v4-flash")
        .expect("deepseek-v4-flash preset present");
    current.show_in_picker = false;
    let current = current.clone();
    assert!(
        !current.show_in_picker,
        "expected deepseek-v4-flash to be hidden from picker for this test"
    );

    let upgrade = current.upgrade.as_ref().expect("upgrade configured");
    available_models
        .iter_mut()
        .find(|preset| preset.model == upgrade.id)
        .expect("upgrade target present")
        .show_in_picker = true;
    assert!(
        should_show_model_migration_prompt(
            &current.model,
            &upgrade.id,
            &config.notices.model_migrations,
            &available_models,
        ),
        "expected migration prompt to be eligible for hidden model"
    );

    let target =
        target_preset_for_upgrade(&available_models, &upgrade.id).expect("upgrade target present");
    let target_description = (!target.description.is_empty()).then(|| target.description.clone());
    let can_opt_out = true;
    let copy = migration_copy_for_models(
        &current.model,
        &upgrade.id,
        upgrade.model_link.clone(),
        upgrade.upgrade_copy.clone(),
        upgrade.migration_markdown.clone(),
        target.display_name.clone(),
        target_description,
        can_opt_out,
    );

    assert_snapshot!(
        "model_migration_prompt_shows_for_hidden_model",
        model_migration_copy_to_plain_text(&copy)
    );
}
