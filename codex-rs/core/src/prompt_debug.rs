use std::sync::Arc;

use codex_exec_server::EnvironmentManager;
use codex_exec_server::ExecServerRuntimePaths;
use codex_extension_api::UserInstructionsProvider;
use codex_login::AuthManager;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::resolve_installation_id;
use crate::session::session::Session;
use crate::session::turn::build_prompt;
use crate::session::turn::built_tools;
use crate::state_db_bridge::StateDbHandle;
use crate::thread_manager::ThreadManager;
use crate::thread_manager::thread_store_from_config;
use codex_extension_api::empty_extension_registry;
use codex_tools::ContextBreakdown;
use codex_tools::compute_context_breakdown;

/// Build the model-visible `input` list for a single debug turn.
#[doc(hidden)]
pub async fn build_prompt_input(
    mut config: Config,
    input: Vec<UserInput>,
    state_db: Option<StateDbHandle>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
) -> CodexResult<Vec<ResponseItem>> {
    config.ephemeral = true;

    let auth_manager =
        AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false).await;

    let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
        config.codex_self_exe.clone(),
        config.codex_linux_sandbox_exe.clone(),
    )?;

    let thread_store = thread_store_from_config(&config, state_db.clone());
    let installation_id = resolve_installation_id(&config.codex_home).await?;
    let thread_manager = ThreadManager::new(
        &config,
        Arc::clone(&auth_manager),
        SessionSource::Exec,
        Arc::new(
            EnvironmentManager::from_codex_home(
                config.codex_home.clone(),
                Some(local_runtime_paths),
            )
            .await
            .map_err(|err| CodexErr::Fatal(err.to_string()))?,
        ),
        empty_extension_registry(),
        user_instructions_provider,
        /*analytics_events_client*/ None,
        thread_store,
        state_db.clone(),
        installation_id,
        /*attestation_provider*/ None,
    );
    let thread = thread_manager.start_thread(config).await?;

    let output = build_prompt_input_from_session(thread.thread.codex.session.as_ref(), input).await;
    let shutdown = thread.thread.shutdown_and_wait().await;
    let _removed = thread_manager.remove_thread(&thread.thread_id).await;

    shutdown?;
    output
}

/// Build the in-process context-floor token breakdown for a single debug turn.
/// Mirrors [`build_prompt_input`] but returns the tokenized breakdown behind a
/// native `/context`.
#[doc(hidden)]
pub async fn build_context_breakdown(
    mut config: Config,
    input: Vec<UserInput>,
    state_db: Option<StateDbHandle>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
) -> CodexResult<ContextBreakdown> {
    config.ephemeral = true;

    let auth_manager =
        AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false).await;

    let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
        config.codex_self_exe.clone(),
        config.codex_linux_sandbox_exe.clone(),
    )?;

    let thread_store = thread_store_from_config(&config, state_db.clone());
    let installation_id = resolve_installation_id(&config.codex_home).await?;
    let thread_manager = ThreadManager::new(
        &config,
        Arc::clone(&auth_manager),
        SessionSource::Exec,
        Arc::new(
            EnvironmentManager::from_codex_home(
                config.codex_home.clone(),
                Some(local_runtime_paths),
            )
            .await
            .map_err(|err| CodexErr::Fatal(err.to_string()))?,
        ),
        empty_extension_registry(),
        user_instructions_provider,
        /*analytics_events_client*/ None,
        thread_store,
        state_db.clone(),
        installation_id,
        /*attestation_provider*/ None,
    );
    let thread = thread_manager.start_thread(config).await?;

    let output =
        build_context_breakdown_from_session(thread.thread.codex.session.as_ref(), input).await;
    let shutdown = thread.thread.shutdown_and_wait().await;
    let _removed = thread_manager.remove_thread(&thread.thread_id).await;

    shutdown?;
    output
}

pub(crate) async fn build_prompt_input_from_session(
    sess: &Session,
    input: Vec<UserInput>,
) -> CodexResult<Vec<ResponseItem>> {
    let (prompt, _window) = build_prompt_and_window_from_session(sess, input).await?;
    Ok(prompt.get_formatted_input())
}

/// Assemble the full would-be model request for a turn (the same `Prompt` that
/// [`build_prompt_input_from_session`] derives its input from) and report the
/// model's context window. The shared core for both the input dump and the
/// `/context` token breakdown.
pub(crate) async fn build_prompt_and_window_from_session(
    sess: &Session,
    input: Vec<UserInput>,
) -> CodexResult<(crate::client_common::Prompt, Option<i64>)> {
    let turn_context = sess
        .new_default_turn_with_sub_id("context-preview".to_string())
        .await;
    let mut prompt_history = sess.clone_history().await;
    let context_items = sess.build_context_update_items(turn_context.as_ref()).await;
    if !context_items.is_empty() {
        prompt_history.record_items(context_items.iter(), turn_context.truncation_policy);
    }

    if !input.is_empty() {
        let response_item = sess.response_item_from_user_input(turn_context.as_ref(), input);
        sess.record_conversation_items(turn_context.as_ref(), std::slice::from_ref(&response_item))
            .await;
    }

    let prompt_input = prompt_history.for_prompt(&turn_context.model_info.input_modalities);
    let router = built_tools(sess, turn_context.as_ref(), &CancellationToken::new()).await?;
    let base_instructions = sess.get_base_instructions().await;
    let prompt = build_prompt(
        prompt_input,
        router.as_ref(),
        turn_context.as_ref(),
        base_instructions,
    );
    let window = turn_context.model_info.resolved_context_window();

    Ok((prompt, window))
}

/// Compute the in-process context-floor token breakdown for a single turn —
/// the data behind a native `/context`. Uses the exact components the request
/// is assembled from (`instructions`, `tools`, `input`), tokenized with
/// `o200k_base`.
pub(crate) async fn build_context_breakdown_from_session(
    sess: &Session,
    input: Vec<UserInput>,
) -> CodexResult<ContextBreakdown> {
    let (prompt, window) = build_prompt_and_window_from_session(sess, input).await?;
    let formatted_input = prompt.get_formatted_input();
    compute_context_breakdown(
        &prompt.base_instructions.text,
        &prompt.tools,
        &formatted_input,
        window,
    )
    .map_err(|e| CodexErr::Fatal(format!("context breakdown serialization failed: {e}")))
}

#[cfg(test)]
#[path = "prompt_debug_tests.rs"]
mod tests;
