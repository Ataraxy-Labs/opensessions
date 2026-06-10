use ratatui_query::{FetchStatus, QueryKeyMatch, QueryStatus, TuiQueryClient};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum TuiKey {
    Dashboard,
    Sessions,
    Agents,
}

impl QueryKeyMatch for TuiKey {
    fn matches_prefix(&self, prefix: &Self) -> bool {
        self == prefix
            || matches!(
                (prefix, self),
                (Self::Dashboard, Self::Sessions) | (Self::Dashboard, Self::Agents)
            )
    }
}

#[test]
fn tui_query_client_exposes_core_query_results() {
    let mut client = TuiQueryClient::default();

    client.observe(TuiKey::Dashboard);
    client.mark_fetching(TuiKey::Dashboard);

    assert!(client.result(&TuiKey::Dashboard).is_initial_loading());

    client.mark_success(TuiKey::Dashboard, 99);

    let result = client.result(&TuiKey::Dashboard);
    assert_eq!(result.status, QueryStatus::Success);
    assert_eq!(result.fetch_status, FetchStatus::Idle);
    assert_eq!(result.updated_at, Some(99));
}

#[test]
fn multiple_tui_query_clients_keep_cache_state_partitioned() {
    let mut left = TuiQueryClient::default();
    let mut right = TuiQueryClient::default();

    left.observe(TuiKey::Dashboard);
    left.mark_success(TuiKey::Dashboard, 1);
    right.observe(TuiKey::Dashboard);
    right.mark_success(TuiKey::Dashboard, 2);

    assert_eq!(left.result(&TuiKey::Dashboard).updated_at, Some(1));
    assert_eq!(right.result(&TuiKey::Dashboard).updated_at, Some(2));
}

#[test]
fn adapter_uses_core_active_invalidation_semantics() {
    let mut client = TuiQueryClient::default();
    client.observe(TuiKey::Dashboard);
    client.observe(TuiKey::Agents);
    client.mark_success(TuiKey::Dashboard, 10);
    client.mark_success(TuiKey::Sessions, 10);
    client.mark_success(TuiKey::Agents, 10);

    let active = client.invalidate(&TuiKey::Dashboard);

    assert_eq!(active, vec![TuiKey::Dashboard, TuiKey::Agents]);
    assert!(client.result(&TuiKey::Dashboard).is_stale);
    assert!(client.result(&TuiKey::Sessions).is_stale);
    assert!(client.result(&TuiKey::Agents).is_stale);
}

#[test]
fn set_active_queries_is_idempotent_and_fetches_stale_active_queries() {
    let mut client = TuiQueryClient::default();
    client.mark_success(TuiKey::Dashboard, 10);
    client.mark_success(TuiKey::Sessions, 10);

    let first = client.set_active_queries([TuiKey::Dashboard, TuiKey::Sessions]);
    assert!(first.is_empty());

    client.invalidate(&TuiKey::Dashboard);

    let second = client.set_active_queries([TuiKey::Dashboard, TuiKey::Sessions]);
    assert_eq!(second, vec![TuiKey::Dashboard, TuiKey::Sessions]);

    client.mark_fetching(TuiKey::Dashboard);
    let third = client.set_active_queries([TuiKey::Dashboard, TuiKey::Sessions]);
    assert_eq!(third, vec![TuiKey::Sessions]);
}
