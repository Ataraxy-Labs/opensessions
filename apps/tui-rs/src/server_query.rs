use crate::generated::protocol::{ClientCommand, ServerQueryKey};
pub use ratatui_query::{FetchStatus, QueryResult, QueryStatus};
use ratatui_query::{QueryKeyMatch, TuiQueryClient};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueryKey {
    Sessions,
    Agents,
    Focus,
    SidebarLayout,
    Settings,
}

impl QueryKeyMatch for QueryKey {
    fn matches_prefix(&self, prefix: &Self) -> bool {
        self == prefix
    }
}

impl From<ServerQueryKey> for QueryKey {
    fn from(key: ServerQueryKey) -> Self {
        match key {
            ServerQueryKey::Sessions => Self::Sessions,
            ServerQueryKey::Agents => Self::Agents,
            ServerQueryKey::Focus => Self::Focus,
            ServerQueryKey::SidebarLayout => Self::SidebarLayout,
            ServerQueryKey::Settings => Self::Settings,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryDefinition {
    pub key: QueryKey,
    pub server_key: ServerQueryKey,
}

impl QueryDefinition {
    pub fn fetch_command(self) -> ClientCommand {
        ClientCommand::FetchQuery {
            key: self.server_key,
        }
    }
}

pub mod queries {
    use super::{QueryDefinition, QueryKey, ServerQueryKey};

    pub fn all() -> [QueryDefinition; 5] {
        [sessions(), agents(), focus(), sidebar_layout(), settings()]
    }

    pub fn sessions() -> QueryDefinition {
        QueryDefinition {
            key: QueryKey::Sessions,
            server_key: ServerQueryKey::Sessions,
        }
    }

    pub fn agents() -> QueryDefinition {
        QueryDefinition {
            key: QueryKey::Agents,
            server_key: ServerQueryKey::Agents,
        }
    }

    pub fn focus() -> QueryDefinition {
        QueryDefinition {
            key: QueryKey::Focus,
            server_key: ServerQueryKey::Focus,
        }
    }

    pub fn sidebar_layout() -> QueryDefinition {
        QueryDefinition {
            key: QueryKey::SidebarLayout,
            server_key: ServerQueryKey::SidebarLayout,
        }
    }

    pub fn settings() -> QueryDefinition {
        QueryDefinition {
            key: QueryKey::Settings,
            server_key: ServerQueryKey::Settings,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerStateQuery {
    client: TuiQueryClient<QueryKey>,
}

impl ServerStateQuery {
    pub fn idle() -> Self {
        Self::default()
    }

    pub fn connecting() -> Self {
        let mut query = Self::idle();
        for definition in queries::all() {
            query.client.observe(definition.key);
            query.client.mark_fetching(definition.key);
        }
        query
    }

    pub fn initial_fetch_commands() -> Vec<ClientCommand> {
        [queries::sessions(), queries::sidebar_layout()]
            .into_iter()
            .map(QueryDefinition::fetch_command)
            .collect()
    }

    pub fn secondary_initial_fetch_commands() -> Vec<ClientCommand> {
        [queries::agents(), queries::focus(), queries::settings()]
            .into_iter()
            .map(QueryDefinition::fetch_command)
            .collect()
    }

    pub fn refetch_commands<I>(&mut self, keys: I) -> Vec<ClientCommand>
    where
        I: IntoIterator<Item = QueryKey>,
    {
        keys.into_iter()
            .filter_map(|key| {
                let definition = definition_for_key(key)?;
                self.client.mark_fetching(definition.key);
                Some(definition.fetch_command())
            })
            .collect()
    }

    pub fn observe_query_success(&mut self, key: ServerQueryKey, updated_at: u64) {
        self.client.mark_success(key.into(), updated_at);
    }

    pub fn observe_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        for definition in queries::all() {
            self.client.mark_error(definition.key, error.clone());
        }
    }

    pub fn observe_invalidation<I>(&mut self, keys: I) -> Vec<QueryKey>
    where
        I: IntoIterator<Item = ServerQueryKey>,
    {
        let mut refetch = Vec::new();
        for key in keys {
            refetch.extend(self.invalidate(&key.into()));
        }
        refetch.sort();
        refetch.dedup();
        refetch
    }

    pub fn invalidate(&mut self, key: &QueryKey) -> Vec<QueryKey> {
        self.client.invalidate(key)
    }

    pub fn result(&self) -> QueryResult<'_> {
        self.client.result(&QueryKey::Sessions)
    }

    pub fn result_for(&self, key: &QueryKey) -> QueryResult<'_> {
        self.client.result(key)
    }
}

fn definition_for_key(key: QueryKey) -> Option<QueryDefinition> {
    Some(match key {
        QueryKey::Sessions => queries::sessions(),
        QueryKey::Agents => queries::agents(),
        QueryKey::Focus => queries::focus(),
        QueryKey::SidebarLayout => queries::sidebar_layout(),
        QueryKey::Settings => queries::settings(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark_all_success(query: &mut ServerStateQuery, ts: u64) {
        for key in [
            ServerQueryKey::Sessions,
            ServerQueryKey::Agents,
            ServerQueryKey::Focus,
            ServerQueryKey::SidebarLayout,
            ServerQueryKey::Settings,
        ] {
            query.observe_query_success(key, ts);
        }
    }

    #[test]
    fn connecting_query_reports_initial_loading_until_first_state() {
        let mut query = ServerStateQuery::connecting();

        assert!(query.result().is_initial_loading());

        query.observe_query_success(ServerQueryKey::Sessions, 42);

        assert_eq!(query.result().status, QueryStatus::Success);
        assert_eq!(query.result().fetch_status, FetchStatus::Idle);
        assert_eq!(query.result().updated_at, Some(42));
    }

    #[test]
    fn successful_state_clears_previous_error() {
        let mut query = ServerStateQuery::connecting();
        query.observe_error("socket closed");

        assert!(query.result().is_error());

        query.observe_query_success(ServerQueryKey::Sessions, 7);

        assert_eq!(query.result().status, QueryStatus::Success);
        assert_eq!(query.result().error, None);
    }

    #[test]
    fn initial_fetch_commands_fetch_each_observed_query() {
        assert_eq!(
            ServerStateQuery::initial_fetch_commands(),
            vec![
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::Sessions,
                },
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::SidebarLayout,
                },
            ]
        );
        assert_eq!(
            ServerStateQuery::secondary_initial_fetch_commands(),
            vec![
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::Agents,
                },
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::Focus,
                },
                ClientCommand::FetchQuery {
                    key: ServerQueryKey::Settings,
                },
            ]
        );
    }

    #[test]
    fn invalidating_many_keys_refetches_each_active_query() {
        let mut query = ServerStateQuery::connecting();
        mark_all_success(&mut query, 10);

        let active = query.observe_invalidation([
            ServerQueryKey::Sessions,
            ServerQueryKey::Agents,
            ServerQueryKey::Focus,
            ServerQueryKey::SidebarLayout,
            ServerQueryKey::Settings,
        ]);

        assert_eq!(
            active,
            vec![
                QueryKey::Sessions,
                QueryKey::Agents,
                QueryKey::Focus,
                QueryKey::SidebarLayout,
                QueryKey::Settings,
            ]
        );
        assert!(query.result_for(&QueryKey::Sessions).is_stale);
        assert!(query.result_for(&QueryKey::Agents).is_stale);
    }

    #[test]
    fn exact_query_invalidation_does_not_invalidate_sibling_queries() {
        let mut query = ServerStateQuery::connecting();
        mark_all_success(&mut query, 10);

        let active = query.invalidate(&QueryKey::Sessions);

        assert_eq!(active, vec![QueryKey::Sessions]);
        assert!(query.result_for(&QueryKey::Sessions).is_stale);
        assert!(!query.result_for(&QueryKey::Agents).is_stale);
    }

    #[test]
    fn server_invalidation_keys_map_to_tui_query_keys() {
        let mut query = ServerStateQuery::connecting();
        mark_all_success(&mut query, 10);

        let active = query.observe_invalidation([ServerQueryKey::Agents]);

        assert_eq!(active, vec![QueryKey::Agents]);
        assert!(query.result_for(&QueryKey::Agents).is_stale);
        assert!(!query.result_for(&QueryKey::Sessions).is_stale);
    }

    #[test]
    fn query_definition_builds_typed_fetch_command() {
        assert_eq!(
            queries::sessions().fetch_command(),
            ClientCommand::FetchQuery {
                key: ServerQueryKey::Sessions,
            }
        );
    }
}
