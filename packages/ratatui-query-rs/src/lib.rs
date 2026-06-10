use std::collections::BTreeSet;
use std::hash::Hash;

pub use query_core::{
    FetchStatus, QueryFilter, QueryKeyMatch, QueryPlan, QueryResult, QueryStatus, RefetchType,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuiQueryClient<K>
where
    K: Clone + Eq + Hash + Ord + QueryKeyMatch,
{
    core: query_core::QueryClient<K>,
    active_keys: BTreeSet<K>,
}

impl<K> Default for TuiQueryClient<K>
where
    K: Clone + Eq + Hash + Ord + QueryKeyMatch,
{
    fn default() -> Self {
        Self {
            core: query_core::QueryClient::default(),
            active_keys: BTreeSet::new(),
        }
    }
}

impl<K> TuiQueryClient<K>
where
    K: Clone + Eq + Hash + Ord + QueryKeyMatch,
{
    pub fn observe(&mut self, key: K) {
        self.core.observe(key);
    }

    pub fn unobserve(&mut self, key: &K) {
        self.core.unobserve(key);
    }

    pub fn mark_fetching(&mut self, key: K) {
        self.core.mark_fetching(key);
    }

    pub fn mark_success(&mut self, key: K, updated_at: u64) {
        self.core.mark_success(key, updated_at);
    }

    pub fn mark_error(&mut self, key: K, error: impl Into<String>) {
        self.core.mark_error(key, error);
    }

    pub fn invalidate(&mut self, prefix: &K) -> Vec<K> {
        self.core.invalidate(prefix)
    }

    pub fn invalidate_queries(
        &mut self,
        filter: QueryFilter<'_, K>,
        refetch_type: RefetchType,
    ) -> QueryPlan<K> {
        self.core.invalidate_queries(filter, refetch_type)
    }

    pub fn set_active_queries<I>(&mut self, keys: I) -> Vec<K>
    where
        I: IntoIterator<Item = K>,
    {
        let next = keys.into_iter().collect::<BTreeSet<_>>();
        for key in self.active_keys.difference(&next) {
            self.core.unobserve(key);
        }
        let mut refetch = Vec::new();
        for key in next.difference(&self.active_keys) {
            if self.core.observe(key.clone()) {
                refetch.push(key.clone());
            }
        }
        for key in next.intersection(&self.active_keys) {
            let result = self.core.result(key);
            if result.is_stale && !result.is_fetching() {
                refetch.push(key.clone());
            }
        }
        refetch.sort();
        self.active_keys = next;
        refetch
    }

    pub fn result(&self, key: &K) -> QueryResult<'_> {
        self.core.result(key)
    }
}

pub type QueryClient<K> = TuiQueryClient<K>;
