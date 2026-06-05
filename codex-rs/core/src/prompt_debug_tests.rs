use super::*;

use std::sync::Arc;

use codex_exec_server::EnvironmentManager;
use codex_login::CodexAuth;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;

use crate::config::test_config;
use crate::thread_manager::ThreadManager;

#[tokio::test]
async fn context_breakdown_from_session_does_not_mutate_live_session() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let mut config = test_config().await;
    config.codex_home = AbsolutePathBuf::from_absolute_path(temp_dir.path())?;
    config.cwd = AbsolutePathBuf::from_absolute_path(temp_dir.path())?;

    let manager = ThreadManager::with_models_provider_and_home_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
        config.codex_home.to_path_buf(),
        Arc::new(EnvironmentManager::default_for_tests()),
    );
    let thread = manager.start_thread(config).await?;
    let sess = thread.thread.codex.session.as_ref();
    let history_before = sess.clone_history().await.raw_items().to_vec();
    let reference_before = sess.reference_context_item().await;

    let breakdown = build_context_breakdown_from_session(
        sess,
        vec![UserInput::Text {
            text: "preview only".to_string(),
            text_elements: Vec::new(),
        }],
    )
    .await?;

    assert!(breakdown.input_tokens > 0);
    assert_eq!(
        history_before,
        sess.clone_history().await.raw_items().to_vec()
    );
    assert_eq!(
        serde_json::to_value(reference_before)?,
        serde_json::to_value(sess.reference_context_item().await)?
    );

    thread.thread.shutdown_and_wait().await?;

    Ok(())
}
