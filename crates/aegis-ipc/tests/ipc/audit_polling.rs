//! Regression coverage for ADR-0135: routine capability polling is not
//! durably audited.
//!
//! The production AT-SPI adapter long-polls `NextAccessibilityAction` every
//! 100 ms and re-queries `GetAccessibilityWindows` every 750 ms. Before
//! ADR-0135 each of those cycles appended one `CapabilityUse` event to the
//! durable hash-chained store, which grew `events-v2.jsonl` without bound
//! until the filesystem filled and the audit append fail-stopped the whole
//! compositor. These tests pin the intended boundary: a timed-out poll and
//! a successful scan query decide nothing and are not journaled, while an
//! actual dispatch delivery, a handler refusal, and an authorization
//! refusal remain durable authority history.

use super::*;

fn accessibility_handler() -> (Arc<TestHandler>, PathBuf) {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    *handler.pair_result.lock().unwrap() = Ok(aegis_ipc::PairedAgent {
        principal: aegis_security::authority::ActorPrincipal::new("prin_a11y").unwrap(),
        credential: "cred_a11y".into(),
        // The adapter's capability ceiling; both endpoints below require
        // these pregrants, so the authorization gate passes and the audit
        // decision is observable.
        pregranted: vec![
            ActorCapability::ObserveWindows,
            ActorCapability::PublishAccessibilityTree,
            ActorCapability::DispatchAccessibilityAction,
        ],
        gated: vec![],
    });
    (handler, path)
}

fn accessibility_client(path: &std::path::Path) -> Client {
    Client::connect_agent_with_timeout(
        path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        None,
        aegis_ipc::AgentHello {
            label: Some("a11y adapter".into()),
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("pairing connects")
}

fn dispatched_request() -> aegis_semantic::SemanticActionRequest {
    aegis_semantic::SemanticActionRequest {
        request_id: 7,
        target: aegis_model::semantic::SemanticObjectId::for_window(WindowId(1)),
        provider_node_id: 3,
        tree_revision: 11,
        action: aegis_model::semantic::SemanticActionIntent::Invoke,
    }
}

#[test]
fn timed_out_action_poll_is_not_audited() {
    let (handler, path) = accessibility_handler();
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = accessibility_client(&path);

    // The adapter's steady state: the poll expires without work.
    *handler.next_action_result.lock().unwrap() = Ok(None);
    for _ in 0..3 {
        assert!(
            client
                .next_accessibility_action(Duration::from_millis(1))
                .expect("poll")
                .is_none()
        );
    }
    assert!(
        handler.capability_uses.lock().unwrap().is_empty(),
        "timed-out long-polls must not reach the durable audit trail"
    );
}

#[test]
fn delivered_action_and_handler_refusal_are_audited() {
    let (handler, path) = accessibility_handler();
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = accessibility_client(&path);

    // A real dispatch is an authority decision: it is audited.
    *handler.next_action_result.lock().unwrap() = Ok(Some(dispatched_request()));
    assert!(
        client
            .next_accessibility_action(Duration::from_millis(1))
            .expect("poll")
            .is_some()
    );
    // A handler-level refusal (quota exhausted, session revoked, ...) is
    // audited as a refused capability use.
    *handler.next_action_result.lock().unwrap() = Err("pending-action quota exhausted".into());
    assert!(
        client
            .next_accessibility_action(Duration::from_millis(1))
            .is_err()
    );

    let uses = handler.capability_uses.lock().unwrap();
    assert_eq!(
        uses.len(),
        2,
        "delivery and refusal are both audited: {uses:?}"
    );
    assert_eq!(uses[0].1, ActorCapability::DispatchAccessibilityAction);
    assert_eq!(uses[0].2, aegis_ipc::CapabilityUseAction::Await);
    assert_eq!(uses[0].3, aegis_ipc::Effect::Applied);
    assert_eq!(
        uses[1].3,
        aegis_ipc::Effect::Refused {
            reason: "capability use refused".into()
        }
    );
}

#[test]
fn accessibility_scan_query_is_not_audited_but_refusal_is() {
    let (handler, path) = accessibility_handler();
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = accessibility_client(&path);

    // Steady state: the periodic scan query succeeds and is not journaled.
    for _ in 0..3 {
        client.accessibility_windows().expect("scan query");
    }
    assert!(
        handler.capability_uses.lock().unwrap().is_empty(),
        "routine scan polling must not reach the durable audit trail"
    );

    // An unauthenticated connection is refused at the authorization gate;
    // that out-of-scope attempt remains durable history.
    let mut anonymous = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");
    assert!(anonymous.accessibility_windows().is_err());
    let refusals = handler.refusals.lock().unwrap();
    assert_eq!(refusals.len(), 1, "authorization refusal is audited");
    assert!(matches!(
        refusals[0].1,
        aegis_ipc::JournalMutation::CapabilityUse { .. }
    ));
}
