use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::{Value, json};
use xai_grok_pager::pi_model_config::{
    PiModelConfig, PiModelConfigSnapshot, PiModelsFile, PiProviderConfig,
};
use xai_grok_pager::views::pi_models::{
    PiModelsModalState, PiModelsOutcome, render_pi_models_modal,
};

use super::PI_GROK_NATIVE_COMMANDS;

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "grok-pi-model-manager-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn model(id: &str) -> PiModelConfig {
    PiModelConfig {
        id: id.to_owned(),
        name: id.to_owned(),
        input: vec!["text".to_owned()],
        context_window: Some(128_000),
        max_tokens: Some(8_192),
        ..PiModelConfig::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn model_config_save_preserves_unknown_fields_and_creates_backup() {
    let temp = TestDir::new("preserve");
    let path = temp.join("models.json");
    fs::write(
        &path,
        r#"{
  "schemaVersion": 7,
  "providers": {
    "demo": {
      "vendorFlag": "keep-me",
      "models": [
        {
          "id": "alpha",
          "name": "Alpha",
          "input": ["text"],
          "customCapability": {"mode": "strict"}
        }
      ]
    }
  }
}
"#,
    )
    .expect("seed models file");

    let mut snapshot = PiModelConfigSnapshot::load_from_path(path.clone()).expect("load");
    snapshot.document.providers.get_mut("demo").unwrap().models[0].name = "Alpha 2".into();
    let report = snapshot.save().expect("save");
    assert!(report.backup.is_some_and(|backup| backup.exists()));

    let value: Value = serde_json::from_slice(&fs::read(path).expect("read saved")).unwrap();
    assert_eq!(value["schemaVersion"], json!(7));
    assert_eq!(value["providers"]["demo"]["vendorFlag"], json!("keep-me"));
    assert_eq!(
        value["providers"]["demo"]["models"][0]["customCapability"],
        json!({"mode": "strict"})
    );
    assert_eq!(
        value["providers"]["demo"]["models"][0]["name"],
        json!("Alpha 2")
    );
}

#[test]
fn model_config_accepts_and_preserves_null_thinking_levels() {
    let temp = TestDir::new("nullable-thinking-levels");
    let path = temp.join("models.json");
    fs::write(
        &path,
        r#"{
  "providers": {
    "demo": {
      "models": [
        {
          "id": "alpha",
          "name": "Alpha",
          "thinkingLevelMap": {
            "off": null,
            "low": "low",
            "medium": null
          }
        }
      ]
    }
  }
}
"#,
    )
    .expect("seed nullable thinking levels");

    let mut snapshot = PiModelConfigSnapshot::load_from_path(path.clone()).expect("load");
    let map = &snapshot.document.providers["demo"].models[0].thinking_level_map;
    assert_eq!(map.get("off"), Some(&None));
    assert_eq!(map.get("low"), Some(&Some("low".to_owned())));
    assert_eq!(map.get("medium"), Some(&None));

    snapshot.document.providers.get_mut("demo").unwrap().models[0].name = "Alpha 2".into();
    snapshot.save().expect("save");
    let value: Value = serde_json::from_slice(&fs::read(path).expect("read saved")).unwrap();
    assert_eq!(
        value["providers"]["demo"]["models"][0]["thinkingLevelMap"],
        json!({"off": null, "low": "low", "medium": null})
    );
}

#[test]
fn model_config_rejects_external_overwrite_and_restores_latest_backup() {
    let temp = TestDir::new("conflict-restore");
    let path = temp.join("models.json");
    fs::write(&path, "{\"providers\":{}}\n").expect("seed");

    let mut first = PiModelConfigSnapshot::load_from_path(path.clone()).expect("load first");
    first.document.providers.insert(
        "first".to_owned(),
        PiProviderConfig {
            models: vec![model("one")],
            ..PiProviderConfig::default()
        },
    );
    first.save().expect("first save");

    let mut stale = PiModelConfigSnapshot::load_from_path(path.clone()).expect("load stale");
    fs::write(&path, "{\"providers\":{\"external\":{\"models\":[]}}}\n").expect("external edit");
    stale
        .document
        .providers
        .insert("local".to_owned(), PiProviderConfig::default());
    assert!(stale.save().is_err());

    let mut current = PiModelConfigSnapshot::load_from_path(path.clone()).expect("reload current");
    current.document.providers.clear();
    current
        .document
        .providers
        .insert("second".to_owned(), PiProviderConfig::default());
    current.save().expect("second save");
    current.restore_latest().expect("restore latest");
    assert!(current.document.providers.contains_key("external"));
}

#[test]
fn model_center_save_returns_reload_and_writes_first_provider_model() {
    let temp = TestDir::new("modal-save");
    let path = temp.join("models.json");
    let snapshot = PiModelConfigSnapshot::load_from_path(path.clone()).expect("empty snapshot");
    let mut state = PiModelsModalState::from_snapshot(snapshot, None);

    assert_eq!(
        state.handle_key(&key(KeyCode::Char('n'))),
        PiModelsOutcome::Changed
    );
    assert_eq!(
        state.handle_key(&key(KeyCode::Tab)),
        PiModelsOutcome::Changed
    );
    assert_eq!(
        state.handle_key(&key(KeyCode::Char('n'))),
        PiModelsOutcome::Changed
    );
    assert_eq!(
        state.handle_key(&key(KeyCode::Char('s'))),
        PiModelsOutcome::Reload
    );

    let saved: PiModelsFile = serde_json::from_slice(&fs::read(path).expect("saved file")).unwrap();
    let provider = saved.providers.get("provider").expect("default provider");
    assert_eq!(provider.models.len(), 1);
    assert_eq!(provider.models[0].id, "model");
}

#[test]
fn model_center_renders_wide_and_narrow_without_overflow() {
    for (width, height) in [(140, 44), (60, 24)] {
        let temp = TestDir::new(&format!("render-{width}"));
        let snapshot = PiModelConfigSnapshot::load_from_path(temp.join("models.json"))
            .expect("empty snapshot");
        let mut state = PiModelsModalState::from_snapshot(snapshot, None);
        state.handle_key(&key(KeyCode::Char('n')));
        state.handle_key(&key(KeyCode::Tab));
        state.handle_key(&key(KeyCode::Char('n')));
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        render_pi_models_modal(&mut buffer, area, &mut state, false);
    }
}

#[test]
fn pi_models_command_is_registered_for_native_composition() {
    assert!(PI_GROK_NATIVE_COMMANDS.contains(&"pi-models"));
    let command = xai_grok_pager::slash::commands::builtin_commands()
        .into_iter()
        .find(|command| command.name() == "pi-models")
        .expect("pi-models command");
    assert_eq!(command.aliases(), &["model-config", "models-config"]);
}

#[test]
fn empty_document_serializes_with_provider_object() {
    let value = serde_json::to_value(PiModelsFile {
        providers: BTreeMap::new(),
        extra: BTreeMap::new(),
    })
    .unwrap();
    assert_eq!(value, json!({"providers": {}}));
}
