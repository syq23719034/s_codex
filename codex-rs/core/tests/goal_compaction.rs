#![allow(clippy::expect_used)]

use anyhow::Result;
use codex_core::StartIfIdleSubmission;
use codex_core::TurnInput;
use codex_core::TurnInputRequest;
use codex_core::TurnStartOptions;
use codex_core::context::ContextualUserFragment;
use codex_core::context::InternalContextSource;
use codex_core::context::InternalModelContextFragment;
use codex_features::Feature;
use codex_protocol::models::ContentItem;
use codex_protocol::protocol::EventMsg;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use test_case::test_case;

#[test_case(false; "local")]
#[test_case(true; "remote_v2")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goal_context_survives_in_request_after_mid_turn_compaction(remote: bool) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let previous_request = "Only complete Stage A, then stop.";
    let objective = "Stage A is complete. Continue implementing and testing Stage B.";
    let goal =
        InternalModelContextFragment::new(InternalContextSource::from_static("goal"), objective);
    let ContentItem::InputText { text: goal_text } = goal.render() else {
        unreachable!("goal renders text");
    };
    // Deliberately omit the current objective from the summary: retention must
    // not depend on the compactor restating it correctly.
    let compaction = if remote {
        responses::sse(vec![
            json!({
                "type": "response.output_item.done",
                "item": {"type": "compaction", "encrypted_content": "Stage A finished."},
            }),
            responses::ev_completed_with_tokens("compacted", /*total_tokens*/ 100),
        ])
    } else {
        responses::sse(vec![
            responses::ev_assistant_message("summary", "Stage A finished."),
            responses::ev_completed_with_tokens("compacted", /*total_tokens*/ 100),
        ])
    };
    let requests = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_assistant_message("stage-a", "Stage A is complete."),
                responses::ev_completed_with_tokens("stage-a", /*total_tokens*/ 100),
            ]),
            responses::sse(vec![
                responses::ev_function_call(
                    "plan-b",
                    "update_plan",
                    &json!({"plan": [{"step": "Implement Stage B", "status": "in_progress"}]})
                        .to_string(),
                ),
                responses::ev_completed_with_tokens("stage-b", /*total_tokens*/ 330_000),
            ]),
            compaction,
            responses::sse(vec![
                responses::ev_assistant_message("continued", "Continue Stage B."),
                responses::ev_completed_with_tokens("continued", /*total_tokens*/ 100),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(move |config| {
            config.update_plan_enabled = true;
            config.model_auto_compact_token_limit = Some(200_000);
            config.features.disable(Feature::TokenBudget).unwrap();
            config.features.disable(Feature::AsyncCompaction).unwrap();
            if !remote {
                config.model_provider.name = "Local compaction regression provider".to_string();
            }
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn(previous_request).await?;
    let submission = test
        .codex
        .start_turn_if_idle(
            TurnInputRequest::new(TurnInput::ResponseItem(ContextualUserFragment::into(goal)))
                .on_start(TurnStartOptions {
                    turn_trigger: Some("goal".to_string()),
                    ..Default::default()
                }),
        )
        .await?;
    assert!(matches!(submission, StartIfIdleSubmission::Started { .. }));
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let requests = requests.requests();
    assert_eq!(requests.len(), 4);
    for request in [&requests[1], &requests[3]] {
        let input = request.input();
        let index_of = |text: &str| {
            input.iter().position(|item| {
                item["role"] == "user"
                    && item["content"]
                        .as_array()
                        .is_some_and(|content| content.iter().any(|part| part["text"] == text))
            })
        };
        let old = index_of(previous_request).expect("older ordinary user request");
        let current = index_of(&goal_text).expect("current Goal must survive compaction");
        assert!(
            old < current,
            "preserve the order of user input and Goal steering"
        );
    }
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
