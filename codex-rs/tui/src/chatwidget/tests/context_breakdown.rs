use super::*;

#[test]
fn context_breakdown_card_groups_tools_and_keeps_messages_aggregate() {
    let cell = context_breakdown_cell(
        context_breakdown_response(vec![
            context_bucket("mcp__codex_apps__google_drive", 10_000),
            context_bucket("mcp__openaiDeveloperDocs", 603),
            context_bucket("update_goal", 390),
            context_bucket("exec_command", 342),
            context_bucket("request_user_input", 287),
            context_bucket("apply_patch", 250),
        ]),
        crate::app_command::ContextBreakdownMode::Compact,
    );

    let rendered = helpers::lines_to_single_string(&cell.display_lines(/*width*/ 100));

    assert!(!rendered.contains("Input by kind"));
    assert!(!rendered.contains("message:developer"));
    assert!(!rendered.contains("message:user"));
    assert!(!rendered.contains("Tool outputs"));
    assert!(rendered.contains("Context usage:"));
    assert!(rendered.contains("Free space:"));
    assert!(rendered.contains("Run /context full to show full details."));
    assert!(rendered.contains("[█░░░░░░░░░░░░░░░░░░░]   3.9%  (10.6k)"));
    assert!(rendered.contains("[███████████████████░]  93.8%  (255.1k)"));
    assert_detail_rows_have_no_bars(&rendered);
    assert_chatwidget_snapshot!("context_breakdown_card", rendered);
}

#[test]
fn context_breakdown_full_card_shows_all_details() {
    let cell = context_breakdown_cell(
        context_breakdown_response(vec![
            context_bucket("mcp__codex_apps__google_drive", 10_000),
            context_bucket("mcp__openaiDeveloperDocs", 603),
            context_bucket("update_goal", 390),
            context_bucket("exec_command", 342),
            context_bucket("request_user_input", 287),
            context_bucket("apply_patch", 250),
        ]),
        crate::app_command::ContextBreakdownMode::Full,
    );

    let rendered = helpers::lines_to_single_string(&cell.display_lines(/*width*/ 100));

    assert!(rendered.contains("Context usage:"));
    assert!(rendered.contains("Free space:"));
    assert!(rendered.contains("[█░░░░░░░░░░░░░░░░░░░]   3.9%  (10.6k)"));
    assert!(rendered.contains("[███████████████████░]  93.8%  (255.1k)"));
    assert!(rendered.contains("google_drive"));
    assert!(!rendered.contains("not shown"));
    assert!(!rendered.contains("Run /context full to show full details."));
    assert_detail_rows_have_no_bars(&rendered);
    assert_chatwidget_snapshot!("context_breakdown_full_card", rendered);
}

#[test]
fn context_breakdown_compact_card_shows_hidden_items() {
    let mut per_tool = Vec::new();
    for i in 1..=14 {
        per_tool.push(context_bucket(&format!("builtin_{i:02}"), 1_500 - i * 10));
    }
    for server in ["alpha", "bravo", "charlie", "delta", "echo"] {
        for tool in 1..=4 {
            per_tool.push(context_bucket(
                &format!("mcp__{server}__tool_{tool}"),
                300 - tool * 10,
            ));
        }
    }
    let cell = context_breakdown_cell(
        context_breakdown_response(per_tool),
        crate::app_command::ContextBreakdownMode::Compact,
    );

    let rendered = helpers::lines_to_single_string(&cell.display_lines(/*width*/ 100));

    assert!(rendered.contains("... 9 more system tools not shown"));
    assert!(rendered.contains("... 2 more MCP servers not shown"));
    assert!(rendered.contains("Run /context full to show full details."));
    assert!(rendered.contains("Free space:"));
    assert_detail_rows_have_no_bars(&rendered);
    assert_chatwidget_snapshot!("context_breakdown_compact_hidden_items", rendered);
}

#[tokio::test]
async fn context_slash_command_preserves_command_line_before_compact_card() {
    let (mut chat, mut rx, mut op_rx) =
        helpers::make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(crate::slash_command::SlashCommand::Context);
    assert_matches!(
        op_rx.try_recv(),
        Ok(Op::ContextBreakdown {
            mode: crate::app_command::ContextBreakdownMode::Compact
        })
    );
    chat.add_context_breakdown_output(
        context_breakdown_response(Vec::new()),
        crate::app_command::ContextBreakdownMode::Compact,
    );

    let lines = inserted_history_lines(&mut rx);
    assert_context_command_line_magenta(&lines, "/context");
    let rendered = helpers::lines_to_single_string(&lines);

    assert!(rendered.contains("/context"));
    assert!(rendered.contains("Context breakdown"));
    assert!(rendered.contains("Run /context full to show full details."));
    assert!(
        rendered
            .find("/context")
            .expect("command line should render")
            < rendered
                .find("Context breakdown")
                .expect("context card should render")
    );
    assert_chatwidget_snapshot!("context_breakdown_compact_command_transcript", rendered);
}

#[tokio::test]
async fn context_full_slash_command_preserves_command_line_before_full_card() {
    let (mut chat, mut rx, mut op_rx) =
        helpers::make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(
        crate::slash_command::SlashCommand::Context,
        "full".to_string(),
        Vec::new(),
    );
    assert_matches!(
        op_rx.try_recv(),
        Ok(Op::ContextBreakdown {
            mode: crate::app_command::ContextBreakdownMode::Full
        })
    );
    chat.add_context_breakdown_output(
        context_breakdown_response(Vec::new()),
        crate::app_command::ContextBreakdownMode::Full,
    );

    let lines = inserted_history_lines(&mut rx);
    assert_context_command_line_magenta(&lines, "/context full");
    let rendered = helpers::lines_to_single_string(&lines);

    assert!(rendered.contains("/context full"));
    assert!(rendered.contains("Context breakdown"));
    assert!(!rendered.contains("Run /context full to show full details."));
    assert!(
        rendered
            .find("/context full")
            .expect("command line should render")
            < rendered
                .find("Context breakdown")
                .expect("context card should render")
    );
    assert_chatwidget_snapshot!("context_breakdown_full_command_transcript", rendered);
}

fn assert_detail_rows_have_no_bars(rendered: &str) {
    let Some((_, details)) = rendered.split_once("Details:") else {
        return;
    };

    assert!(!details.contains('['));
    assert!(!details.contains(']'));
}

fn context_breakdown_cell(
    breakdown: codex_app_server_protocol::ContextBreakdownResponse,
    mode: crate::app_command::ContextBreakdownMode,
) -> ContextBreakdownHistoryCell {
    ContextBreakdownHistoryCell { breakdown, mode }
}

fn inserted_history_lines(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> Vec<ratatui::text::Line<'static>> {
    helpers::drain_insert_history(rx)
        .into_iter()
        .flat_map(|lines| lines.into_iter())
        .collect::<Vec<_>>()
}

fn assert_context_command_line_magenta(lines: &[ratatui::text::Line<'static>], command_line: &str) {
    let command = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == command_line)
        .unwrap_or_else(|| panic!("expected {command_line} command line"));

    assert_eq!(command.style.fg, Some(ratatui::style::Color::Magenta));
}

fn context_breakdown_response(
    per_tool: Vec<codex_app_server_protocol::ContextTokenBucket>,
) -> codex_app_server_protocol::ContextBreakdownResponse {
    codex_app_server_protocol::ContextBreakdownResponse {
        system_prompt_tokens: 1_200,
        builtin_tools_tokens: 1_069,
        mcp_tools_tokens: 10_603,
        skills_tokens: 600,
        input_tokens: 3_400,
        total_tokens: 16_872,
        context_window: Some(272_000),
        per_tool,
        per_input_kind: vec![
            context_bucket("message:developer", 2_400),
            context_bucket("message:user", 1_000),
        ],
        per_tool_output: vec![context_bucket("exec_command", 500)],
    }
}

fn context_bucket(label: &str, tokens: u64) -> codex_app_server_protocol::ContextTokenBucket {
    codex_app_server_protocol::ContextTokenBucket {
        label: label.to_string(),
        tokens,
        count: 1,
    }
}
