use super::*;

#[test]
fn context_breakdown_card_groups_tools_and_keeps_messages_aggregate() {
    let cell = ContextBreakdownHistoryCell {
        breakdown: codex_app_server_protocol::ContextBreakdownResponse {
            system_prompt_tokens: 1_200,
            builtin_tools_tokens: 1_069,
            mcp_tools_tokens: 10_603,
            input_tokens: 4_000,
            total_tokens: 16_872,
            context_window: Some(272_000),
            per_tool: vec![
                context_bucket("mcp__codex_apps__google_drive", 10_000),
                context_bucket("mcp__openaiDeveloperDocs", 603),
                context_bucket("update_goal", 390),
                context_bucket("exec_command", 342),
                context_bucket("request_user_input", 287),
                context_bucket("apply_patch", 250),
            ],
            per_input_kind: vec![
                context_bucket("message:developer", 3_000),
                context_bucket("message:user", 1_000),
            ],
            per_tool_output: vec![context_bucket("exec_command", 500)],
        },
    };

    let rendered = helpers::lines_to_single_string(&cell.display_lines(/*width*/ 100));

    assert!(!rendered.contains("Input by kind"));
    assert!(!rendered.contains("message:developer"));
    assert!(!rendered.contains("message:user"));
    assert!(!rendered.contains("Tool outputs"));
    assert_chatwidget_snapshot!("context_breakdown_card", rendered);
}

fn context_bucket(label: &str, tokens: u64) -> codex_app_server_protocol::ContextTokenBucket {
    codex_app_server_protocol::ContextTokenBucket {
        label: label.to_string(),
        tokens,
        count: 1,
    }
}
