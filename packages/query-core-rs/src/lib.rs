use std::collections::HashMap;
use std::hash::Hash;

pub trait QueryKeyMatch {
    fn matches_prefix(&self, prefix: &Self) -> bool;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchMode {
    Exact,
    #[default]
    Prefix,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RefetchType {
    None,
    #[default]
    Active,
    Inactive,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryFilter<'a, K> {
    pub key: Option<&'a K>,
    pub match_mode: MatchMode,
    pub active: Option<bool>,
    pub stale: Option<bool>,
    pub fetch_status: Option<FetchStatus>,
}

impl<K> Default for QueryFilter<'_, K> {
    fn default() -> Self {
        Self {
            key: None,
            match_mode: MatchMode::Prefix,
            active: None,
            stale: None,
            fetch_status: None,
        }
    }
}

impl<'a, K> QueryFilter<'a, K> {
    pub fn prefix(key: &'a K) -> Self {
        Self {
            key: Some(key),
            match_mode: MatchMode::Prefix,
            ..Self::default()
        }
    }

    pub fn exact(key: &'a K) -> Self {
        Self {
            key: Some(key),
            match_mode: MatchMode::Exact,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryPlan<K> {
    pub matched: Vec<K>,
    pub refetch: Vec<K>,
}

impl<K> Default for QueryPlan<K> {
    fn default() -> Self {
        Self {
            matched: Vec::new(),
            refetch: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryClient<K>
where
    K: Clone + Eq + Hash + Ord + QueryKeyMatch,
{
    queries: HashMap<K, QueryEntry>,
}

impl<K> Default for QueryClient<K>
where
    K: Clone + Eq + Hash + Ord + QueryKeyMatch,
{
    fn default() -> Self {
        Self {
            queries: HashMap::new(),
        }
    }
}

impl<K> QueryClient<K>
where
    K: Clone + Eq + Hash + Ord + QueryKeyMatch,
{
    pub fn observe(&mut self, key: K) -> bool {
        let entry = self.entry_mut(key);
        entry.observers = entry.observers.saturating_add(1);
        entry.should_fetch()
    }

    pub fn unobserve(&mut self, key: &K) {
        if let Some(entry) = self.queries.get_mut(key) {
            entry.observers = entry.observers.saturating_sub(1);
        }
    }

    pub fn mark_fetching(&mut self, key: K) {
        let entry = self.entry_mut(key);
        entry.fetch_status = FetchStatus::Fetching;
        if entry.updated_at.is_none() {
            entry.status = QueryStatus::Pending;
            entry.error = None;
        }
    }

    pub fn mark_success(&mut self, key: K, updated_at: u64) {
        let entry = self.entry_mut(key);
        entry.status = QueryStatus::Success;
        entry.fetch_status = FetchStatus::Idle;
        entry.updated_at = Some(updated_at);
        entry.error = None;
        entry.invalidated = false;
    }

    pub fn mark_error(&mut self, key: K, error: impl Into<String>) {
        let entry = self.entry_mut(key);
        entry.status = QueryStatus::Error;
        entry.fetch_status = FetchStatus::Idle;
        entry.error = Some(error.into());
    }

    /// Mark matching queries stale and return active matching queries that should refetch now.
    ///
    /// This mirrors TanStack Query's useful default: invalidation is broad by key prefix,
    /// but only active observers refetch immediately.
    pub fn invalidate(&mut self, prefix: &K) -> Vec<K> {
        self.invalidate_queries(QueryFilter::prefix(prefix), RefetchType::Active)
            .refetch
    }

    pub fn invalidate_queries(
        &mut self,
        filter: QueryFilter<'_, K>,
        refetch_type: RefetchType,
    ) -> QueryPlan<K> {
        let mut plan = QueryPlan::default();
        for (key, entry) in &mut self.queries {
            if query_matches(key, entry, &filter) {
                entry.invalidated = true;
                plan.matched.push(key.clone());
                if should_refetch(entry, refetch_type) {
                    plan.refetch.push(key.clone());
                }
            }
        }
        plan.matched.sort();
        plan.refetch.sort();
        plan
    }

    pub fn result(&self, key: &K) -> QueryResult<'_> {
        self.queries
            .get(key)
            .map(QueryEntry::result)
            .unwrap_or_else(QueryResult::pending_idle)
    }

    fn entry_mut(&mut self, key: K) -> &mut QueryEntry {
        self.queries.entry(key).or_default()
    }
}

fn query_matches<K>(key: &K, entry: &QueryEntry, filter: &QueryFilter<'_, K>) -> bool
where
    K: Eq + QueryKeyMatch,
{
    if let Some(filter_key) = filter.key {
        match filter.match_mode {
            MatchMode::Exact if key != filter_key => return false,
            MatchMode::Prefix if !key.matches_prefix(filter_key) => return false,
            _ => {}
        }
    }
    if let Some(active) = filter.active
        && entry.is_active() != active
    {
        return false;
    }
    if let Some(stale) = filter.stale
        && entry.is_stale() != stale
    {
        return false;
    }
    if let Some(fetch_status) = filter.fetch_status
        && entry.fetch_status != fetch_status
    {
        return false;
    }
    true
}

fn should_refetch(entry: &QueryEntry, refetch_type: RefetchType) -> bool {
    if entry.fetch_status == FetchStatus::Fetching {
        return false;
    }
    match refetch_type {
        RefetchType::None => false,
        RefetchType::Active => entry.is_active(),
        RefetchType::Inactive => !entry.is_active(),
        RefetchType::All => true,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueryEntry {
    status: QueryStatus,
    fetch_status: FetchStatus,
    updated_at: Option<u64>,
    error: Option<String>,
    invalidated: bool,
    observers: usize,
}

impl Default for QueryEntry {
    fn default() -> Self {
        Self {
            status: QueryStatus::Pending,
            fetch_status: FetchStatus::Idle,
            updated_at: None,
            error: None,
            invalidated: false,
            observers: 0,
        }
    }
}

impl QueryEntry {
    fn is_active(&self) -> bool {
        self.observers > 0
    }

    fn is_stale(&self) -> bool {
        self.updated_at.is_none() || self.invalidated
    }

    fn should_fetch(&self) -> bool {
        self.is_stale() && self.fetch_status != FetchStatus::Fetching
    }

    fn result(&self) -> QueryResult<'_> {
        QueryResult {
            status: self.status,
            fetch_status: self.fetch_status,
            updated_at: self.updated_at,
            error: self.error.as_deref(),
            is_stale: self.is_stale(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryStatus {
    Pending,
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchStatus {
    Idle,
    Fetching,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryResult<'a> {
    pub status: QueryStatus,
    pub fetch_status: FetchStatus,
    pub updated_at: Option<u64>,
    pub error: Option<&'a str>,
    pub is_stale: bool,
}

impl QueryResult<'_> {
    fn pending_idle() -> Self {
        Self {
            status: QueryStatus::Pending,
            fetch_status: FetchStatus::Idle,
            updated_at: None,
            error: None,
            is_stale: true,
        }
    }

    pub fn is_initial_loading(&self) -> bool {
        self.status == QueryStatus::Pending && self.fetch_status == FetchStatus::Fetching
    }

    pub fn is_error(&self) -> bool {
        self.status == QueryStatus::Error
    }

    pub fn is_success(&self) -> bool {
        self.status == QueryStatus::Success
    }

    pub fn is_fetching(&self) -> bool {
        self.fetch_status == FetchStatus::Fetching
    }

    pub fn is_refetching(&self) -> bool {
        self.status != QueryStatus::Pending && self.is_fetching()
    }
}
