use std::{collections::HashMap, sync::Arc};

use teleark_runtime::ManagedVaultFile;

/// Retaining the source makes all subsequent edits copy-on-write and invalidates
/// this projection without scanning unchanged catalog rows during rendering.
#[derive(Default)]
pub(crate) struct ManagedProjection {
    source: Arc<Vec<ManagedVaultFile>>,
    query: String,
    rows: Arc<Vec<usize>>,
    by_message: HashMap<i64, usize>,
}

impl ManagedProjection {
    pub(crate) fn rows(
        &mut self,
        source: &Arc<Vec<ManagedVaultFile>>,
        query: &str,
    ) -> Arc<Vec<usize>> {
        if !Arc::ptr_eq(source, &self.source) || query != self.query {
            self.source = source.clone();
            self.query = query.into();
            self.by_message.clear();
            self.rows = Arc::new(
                source
                    .iter()
                    .enumerate()
                    .filter_map(|(index, file)| {
                        if query.is_empty() || file.logical_name.to_lowercase().contains(query) {
                            self.by_message.insert(file.manifest_message_id, index);
                            Some(index)
                        } else {
                            None
                        }
                    })
                    .collect(),
            );
        }
        self.rows.clone()
    }

    pub(crate) fn selected(&self, message: Option<i64>) -> Option<&ManagedVaultFile> {
        self.by_message
            .get(&message?)
            .and_then(|index| self.source.get(*index))
    }
}
