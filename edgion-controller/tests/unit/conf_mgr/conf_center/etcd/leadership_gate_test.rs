use super::*;

#[test]
fn gate_blocks_when_not_leader() {
    let st = LeaderState::new(Duration::from_secs(15));
    let gate = LeadershipGate::from_state(st.clone());
    assert!(gate.check().is_err());
    st.set_leader(true);
    st.record_keepalive_now();
    assert!(gate.check().is_ok());
}

#[test]
fn gate_blocks_when_lease_stale_even_if_flag_set() {
    let st = LeaderState::new(Duration::from_millis(50));
    st.set_leader(true);
    st.record_keepalive_at(Instant::now() - Duration::from_millis(200), wall_now() - 200);
    let gate = LeadershipGate::from_state(st.clone());
    assert!(
        gate.check().is_err(),
        "a stale lease must fail the gate even with is_leader=true"
    );
}

#[test]
fn handle_parses_leader_info() {
    let st = LeaderState::new(Duration::from_secs(15));
    st.set_current_leader(Some(LeaderInfo {
        identity: "ctrl-a".into(),
        grpc_addr: "10.0.0.11:9090".into(),
        admin_addr: "10.0.0.11:8080".into(),
    }));
    let h = LeaderHandle::from_state(st);
    assert_eq!(h.current_leader().unwrap().admin_addr, "10.0.0.11:8080");
}
