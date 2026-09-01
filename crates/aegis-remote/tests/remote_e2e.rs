//! End-to-end integration tests for UIP remote sessions, client SDK, and dead-man drain.

use aegis_model::interaction_domain::{InteractionDomainId, InteractionPrincipalId, SeatId};
use aegis_model::uip::*;
use aegis_remote::{AuthToken, RemoteSession, SessionError, SessionLifecycle, UipClient};
use std::time::Duration;

#[test]
fn test_remote_session_lifecycle_and_drain() {
    let seat = SeatId(10);
    let principal = InteractionPrincipalId(5);
    let token = AuthToken::random();
    let domain = InteractionDomainId(2);

    let mut session = RemoteSession::new(
        token,
        principal,
        seat,
        vec![domain],
        Duration::from_millis(200),
    );

    let client = UipClient::new(seat, principal, token);

    // 1. Ingest a pointer button press
    let press_frame = client.trigger(MonotonicTimestampUs(100_000), 1, true);
    session
        .ingest_frame(&press_frame)
        .expect("ingest should succeed");

    assert_eq!(session.tracker.active_triggers.len(), 1);
    assert_eq!(session.state, SessionLifecycle::Active);

    // 2. Advance time within the heartbeat window (50ms elapsed) -> no timeout
    let drain_none = session.check_heartbeat(MonotonicTimestampUs(150_000));
    assert!(drain_none.is_none());
    assert_eq!(session.state, SessionLifecycle::Active);

    // 3. Advance time beyond the 200ms threshold (250ms elapsed) -> dead-man switch fires!
    let drain_frames = session
        .check_heartbeat(MonotonicTimestampUs(350_000))
        .expect("dead-man switch must trigger");

    assert_eq!(session.state, SessionLifecycle::Terminated);
    assert_eq!(drain_frames.len(), 1);

    // Verify the drain frame automatically released the button
    let frame = &drain_frames[0];
    assert_eq!(frame.seat_id, seat);
    assert!(frame.flags.synthetic_drain);
    if let ActionPayload::Discrete(DiscreteTransition::Trigger {
        trigger_id, state, ..
    }) = &frame.payload
    {
        assert_eq!(*trigger_id, 1);
        assert!(!*state); // Released!
    } else {
        panic!("expected trigger release frame");
    }

    // 4. Ingesting to a terminated session must fail closed
    let next_frame = client.motion_2d(MonotonicTimestampUs(360_000), 0.0, 0.0, 10.0, 10.0, 1.0);
    assert!(matches!(
        session.ingest_frame(&next_frame),
        Err(SessionError::SessionExpired)
    ));
}

#[test]
fn test_unauthorized_principal_rejection() {
    let seat = SeatId(10);
    let principal = InteractionPrincipalId(5);
    let token = AuthToken::random();
    let mut session = RemoteSession::new(
        token,
        principal,
        seat,
        vec![InteractionDomainId(1)],
        Duration::from_millis(500),
    );

    let malicious_client = UipClient::new(seat, InteractionPrincipalId(999), token);
    let frame = malicious_client.trigger(MonotonicTimestampUs(100), 1, true);

    assert!(matches!(
        session.ingest_frame(&frame),
        Err(SessionError::PrincipalMismatch(_))
    ));
}
