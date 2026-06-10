use query_core::{FetchStatus, QueryClient, QueryFilter, QueryKeyMatch, QueryStatus, RefetchType};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum TestKey {
    Root,
    List,
    Detail(u8),
    Settings,
}

impl QueryKeyMatch for TestKey {
    fn matches_prefix(&self, prefix: &Self) -> bool {
        self == prefix
            || matches!(
                (prefix, self),
                (Self::Root, Self::List)
                    | (Self::Root, Self::Detail(_))
                    | (Self::Root, Self::Settings)
                    | (Self::List, Self::Detail(_))
            )
    }
}

#[test]
fn fetching_without_cached_data_is_initial_loading() {
    let mut client = QueryClient::default();
    assert!(client.observe(TestKey::Root));
    client.mark_fetching(TestKey::Root);

    let result = client.result(&TestKey::Root);

    assert_eq!(result.status, QueryStatus::Pending);
    assert_eq!(result.fetch_status, FetchStatus::Fetching);
    assert!(result.is_initial_loading());
    assert!(result.is_stale);
}

#[test]
fn success_clears_error_and_staleness() {
    let mut client = QueryClient::default();
    client.mark_error(TestKey::Root, "socket closed");

    assert!(client.result(&TestKey::Root).is_error());

    client.mark_success(TestKey::Root, 7);

    let result = client.result(&TestKey::Root);
    assert_eq!(result.status, QueryStatus::Success);
    assert!(result.is_success());
    assert_eq!(result.fetch_status, FetchStatus::Idle);
    assert_eq!(result.updated_at, Some(7));
    assert_eq!(result.error, None);
    assert!(!result.is_stale);
}

#[test]
fn invalidation_marks_matching_queries_stale_but_refetches_only_active_matches() {
    let mut client = QueryClient::default();
    client.observe(TestKey::Root);
    client.observe(TestKey::Detail(1));
    client.mark_success(TestKey::Root, 10);
    client.mark_success(TestKey::List, 10);
    client.mark_success(TestKey::Detail(1), 10);
    client.mark_success(TestKey::Settings, 10);

    let active = client.invalidate(&TestKey::Root);

    assert_eq!(active, vec![TestKey::Root, TestKey::Detail(1)]);
    assert!(client.result(&TestKey::Root).is_stale);
    assert!(client.result(&TestKey::List).is_stale);
    assert!(client.result(&TestKey::Detail(1)).is_stale);
    assert!(client.result(&TestKey::Settings).is_stale);
}

#[test]
fn narrower_invalidation_does_not_mark_siblings_stale() {
    let mut client = QueryClient::default();
    client.observe(TestKey::List);
    client.observe(TestKey::Settings);
    client.mark_success(TestKey::List, 10);
    client.mark_success(TestKey::Settings, 10);

    let active = client.invalidate(&TestKey::List);

    assert_eq!(active, vec![TestKey::List]);
    assert!(client.result(&TestKey::List).is_stale);
    assert!(!client.result(&TestKey::Settings).is_stale);
}

#[test]
fn unobserved_invalidated_queries_do_not_refetch_until_observed_later() {
    let mut client = QueryClient::default();
    client.observe(TestKey::List);
    client.unobserve(&TestKey::List);
    client.mark_success(TestKey::List, 10);

    let active = client.invalidate(&TestKey::List);

    assert!(active.is_empty());
    assert!(client.result(&TestKey::List).is_stale);

    assert!(client.observe(TestKey::List));
}

#[test]
fn exact_filter_matches_only_the_requested_key() {
    let mut client = QueryClient::default();
    client.observe(TestKey::Root);
    client.observe(TestKey::List);
    client.mark_success(TestKey::Root, 10);
    client.mark_success(TestKey::List, 10);

    let plan = client.invalidate_queries(QueryFilter::exact(&TestKey::Root), RefetchType::All);

    assert_eq!(plan.matched, vec![TestKey::Root]);
    assert_eq!(plan.refetch, vec![TestKey::Root]);
    assert!(client.result(&TestKey::Root).is_stale);
    assert!(!client.result(&TestKey::List).is_stale);
}

#[test]
fn refetch_type_controls_which_matching_queries_refetch() {
    let mut client = QueryClient::default();
    client.observe(TestKey::Root);
    client.mark_success(TestKey::Root, 10);
    client.mark_success(TestKey::List, 10);

    let none = client.invalidate_queries(QueryFilter::prefix(&TestKey::Root), RefetchType::None);
    assert_eq!(none.matched, vec![TestKey::Root, TestKey::List]);
    assert!(none.refetch.is_empty());

    client.mark_success(TestKey::Root, 11);
    client.mark_success(TestKey::List, 11);
    let inactive =
        client.invalidate_queries(QueryFilter::prefix(&TestKey::Root), RefetchType::Inactive);
    assert_eq!(inactive.refetch, vec![TestKey::List]);

    client.mark_success(TestKey::Root, 12);
    client.mark_success(TestKey::List, 12);
    let all = client.invalidate_queries(QueryFilter::prefix(&TestKey::Root), RefetchType::All);
    assert_eq!(all.refetch, vec![TestKey::Root, TestKey::List]);
}

#[test]
fn already_fetching_queries_are_not_queued_for_duplicate_refetch() {
    let mut client = QueryClient::default();
    client.observe(TestKey::Root);
    client.observe(TestKey::List);
    client.mark_success(TestKey::Root, 10);
    client.mark_success(TestKey::List, 10);
    client.mark_fetching(TestKey::Root);

    let plan = client.invalidate_queries(QueryFilter::prefix(&TestKey::Root), RefetchType::All);

    assert_eq!(plan.refetch, vec![TestKey::List]);
    assert!(client.result(&TestKey::Root).is_refetching());
}
