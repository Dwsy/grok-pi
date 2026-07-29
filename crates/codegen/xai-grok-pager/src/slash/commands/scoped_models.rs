//! `/scoped-models` — configure the active session's Pi-style model scope.

use agent_client_protocol as acp;

use crate::acp::model_state::{ModelState, ScopedModel};
use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct ScopedModelsCommand;

impl SlashCommand for ScopedModelsCommand {
    fn name(&self) -> &str {
        "scoped-models"
    }

    fn description(&self) -> &str {
        "Limit model cycling to a session-only model scope"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/scoped-models [all|provider/model[:effort], ...]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        false
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("all | provider/model[:effort], ...")
    }

    fn suggest_args(&self, ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }
        let mut items = vec![ArgItem {
            display: if ctx.models.has_scoped_models() {
                "All models".into()
            } else {
                "All models (active)".into()
            },
            match_text: "all clear reset every model".into(),
            insert_text: "all".into(),
            description: "Clear the session scope".into(),
        }];
        items.extend(ctx.models.available.iter().map(|(id, info)| {
            let token = model_token(id, info);
            let scoped = ctx.models.is_model_scoped(id);
            ArgItem {
                display: format!(
                    "{}{}",
                    if info.name.trim().is_empty() {
                        token.clone()
                    } else {
                        info.name.clone()
                    },
                    if scoped { " (scoped)" } else { "" }
                ),
                match_text: format!("{} {} {}", token, id.0, info.name),
                insert_text: token,
                description: "Add to comma-separated scope".into(),
            }
        }));
        Some(items)
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Action(Action::OpenScopedModelsPicker);
        }
        if trimmed.eq_ignore_ascii_case("list") {
            return CommandResult::Message(format_scope(ctx.models));
        }
        if matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "all" | "clear" | "reset"
        ) {
            return CommandResult::Action(Action::SetScopedModels(Vec::new()));
        }

        let mut scoped = Vec::new();
        for raw_pattern in trimmed.split(',') {
            let pattern = raw_pattern.trim();
            if pattern.is_empty() {
                return CommandResult::Error("empty scoped-model pattern".into());
            }
            let (model_text, effort_text) = split_effort_suffix(ctx.models, pattern);
            let Some(model_id) = ctx
                .models
                .resolve_by_name_or_id(model_text)
            else {
                return CommandResult::Error(format!("Unknown model in scope: {model_text}"));
            };
            let effort = match effort_text {
                Some(token) => match ctx.models.resolve_effort_for_model(&model_id, token) {
                    Ok(effort) => Some(effort),
                    Err(error) => return CommandResult::Error(error.message()),
                },
                None => None,
            };
            scoped.push(ScopedModel::new(model_id, effort));
        }
        CommandResult::Action(Action::SetScopedModels(scoped))
    }
}

fn split_effort_suffix<'a>(models: &ModelState, pattern: &'a str) -> (&'a str, Option<&'a str>) {
    let Some((model_text, suffix)) = pattern.rsplit_once(':') else {
        return (pattern, None);
    };
    let model_text = model_text.trim();
    let suffix = suffix.trim();
    if model_text.is_empty() || suffix.is_empty() {
        return (pattern, None);
    }
    let Some(model_id) = models.resolve_by_name_or_id(model_text) else {
        return (pattern, None);
    };
    if models.resolve_effort_for_model(&model_id, suffix).is_ok() {
        (model_text, Some(suffix))
    } else {
        // Preserve catalog ids containing ':' as a normal model token. If the
        // prefix is a real model, the caller will surface the effort error.
        if models.available.contains_key(&model_id) {
            (model_text, Some(suffix))
        } else {
            (pattern, None)
        }
    }
}

fn format_scope(models: &ModelState) -> String {
    if !models.has_scoped_models() {
        return "Scoped models: all available models".into();
    }
    let rows = models
        .scoped_models()
        .iter()
        .map(|entry| {
            let name = models
                .available
                .get(&entry.model_id)
                .map(|info| model_token(&entry.model_id, info))
                .unwrap_or_else(|| entry.model_id.0.to_string());
            match entry.effort.as_ref() {
                Some(effort) => format!("{name}:{effort}"),
                None => name,
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Scoped models ({}): {rows}", models.scoped_models().len())
}

fn model_token(id: &acp::ModelId, info: &acp::ModelInfo) -> String {
    let raw = id.0.as_ref();
    if let Some((provider, model_id)) = raw.split_once("::") {
        return format!("{provider}/{model_id}");
    }
    let provider = info
        .meta
        .as_ref()
        .and_then(|meta| meta.get("provider"))
        .and_then(|value| value.as_str());
    let model_id = info
        .meta
        .as_ref()
        .and_then(|meta| meta.get("modelId"))
        .and_then(|value| value.as_str())
        .unwrap_or(raw);
    match provider {
        Some(provider) if !provider.is_empty() => format!("{provider}/{model_id}"),
        _ => model_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    static EMPTY_BUNDLE: crate::app::bundle::BundleState = crate::app::bundle::BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn models() -> ModelState {
        let mut state = ModelState::default();
        let plain = acp::ModelId::new(Arc::from("test::plain"));
        state.available.insert(
            plain.clone(),
            acp::ModelInfo::new(plain, "Plain".to_string()),
        );
        let reasoning = acp::ModelId::new(Arc::from("test::reasoning"));
        state.available.insert(
            reasoning.clone(),
            acp::ModelInfo::new(reasoning, "Reasoning".to_string()).meta(
                serde_json::json!({
                    "supportsReasoningEffort": true,
                    "reasoningEfforts": [
                        { "id": "low", "value": "low", "label": "Low" },
                        { "id": "high", "value": "high", "label": "High" }
                    ]
                })
                .as_object()
                .cloned(),
            ),
        );
        state
    }

    fn exec(models: &ModelState) -> CommandExecCtx<'_> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &EMPTY_BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        }
    }

    #[test]
    fn empty_args_opens_native_scope_picker() {
        let models = models();
        let mut ctx = exec(&models);
        assert!(matches!(
            ScopedModelsCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenScopedModelsPicker)
        ));
    }

    #[test]
    fn list_reports_all_models_by_default() {
        let models = models();
        let mut ctx = exec(&models);
        assert!(matches!(
            ScopedModelsCommand.run(&mut ctx, "list"),
            CommandResult::Message(text) if text == "Scoped models: all available models"
        ));
    }

    #[test]
    fn parses_comma_patterns_and_effort() {
        let models = models();
        let mut ctx = exec(&models);
        let result = ScopedModelsCommand.run(&mut ctx, "test/plain, test/reasoning:high");
        let CommandResult::Action(Action::SetScopedModels(entries)) = result else {
            panic!("expected scoped action");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model_id.0.as_ref(), "test::plain");
        assert_eq!(entries[1].model_id.0.as_ref(), "test::reasoning");
        assert_eq!(entries[1].effort, Some("high".parse().unwrap()));
    }

    #[test]
    fn all_clears_scope() {
        let models = models();
        let mut ctx = exec(&models);
        assert!(matches!(
            ScopedModelsCommand.run(&mut ctx, "all"),
            CommandResult::Action(Action::SetScopedModels(entries)) if entries.is_empty()
        ));
    }

    #[test]
    fn invalid_model_or_effort_is_rejected() {
        let models = models();
        let mut ctx = exec(&models);
        assert!(matches!(
            ScopedModelsCommand.run(&mut ctx, "missing"),
            CommandResult::Error(text) if text.contains("Unknown model")
        ));
        assert!(matches!(
            ScopedModelsCommand.run(&mut ctx, "test/reasoning:none"),
            CommandResult::Error(text) if text.contains("unknown effort level")
        ));
    }
}
