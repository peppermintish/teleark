//! Concise shell presentation; detailed phases and timestamps stay in the inspector.
use std::sync::{Arc, Weak};
use teleark_runtime::{ChannelDownloadSnapshot, TransferSnapshotView, VaultTransferSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StatusRates {
    pub download: Option<u64>,
    pub upload: Option<u64>,
    pub cleanup: Option<(u64, teleark_runtime::ChannelDownloadCleanupPhase)>,
}

struct CachedRates {
    account: Option<i64>,
    native: Weak<[Arc<ChannelDownloadSnapshot>]>,
    vault: Weak<[Arc<VaultTransferSnapshot>]>,
    revisions: (u64, u64),
    rates: StatusRates,
}

#[derive(Default)]
pub(super) struct RateCache {
    cached: Option<CachedRates>,
    #[cfg(test)]
    rebuilds: usize,
}

impl RateCache {
    pub fn read(
        &mut self,
        account: Option<i64>,
        native: &TransferSnapshotView<ChannelDownloadSnapshot>,
        vault: &TransferSnapshotView<VaultTransferSnapshot>,
    ) -> StatusRates {
        let revisions = (native.revision, vault.revision);
        let native_key = Arc::downgrade(&native.items);
        let vault_key = Arc::downgrade(&vault.items);
        if let Some(cached) = &self.cached
            && cached.account == account
            && cached.revisions == revisions
            && cached.native.ptr_eq(&native_key)
            && cached.vault.ptr_eq(&vault_key)
        {
            return cached.rates;
        }
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
        let mut rates = StatusRates {
            download: Some(0),
            upload: Some(0),
            cleanup: None,
        };
        for item in native
            .items
            .iter()
            .filter(|item| account.is_some() && item.account_id == account)
        {
            if let Some(cleanup) = item.cleanup
                && (rates.cleanup.is_none()
                    || matches!(
                        cleanup.phase,
                        teleark_runtime::ChannelDownloadCleanupPhase::Failed(_)
                    ))
            {
                rates.cleanup = Some((item.id, cleanup.phase));
            }
            if item.state != teleark_runtime::ChannelDownloadState::Running {
                continue;
            }
            rates.download = rates
                .download
                .zip(item.current_bytes_per_second)
                .map(|(sum, rate)| sum.saturating_add(rate));
        }
        for item in vault.items.iter().filter(|item| {
            Some(item.account_id) == account
                && item.state == teleark_runtime::VaultTransferState::Running
        }) {
            // A restored/not-yet-measured task cannot invent a zero-speed sample.
            let measured = item
                .average_bytes_per_second
                .map(|_| item.telemetry.goodput_bytes_per_second);
            let total = match item.direction {
                teleark_runtime::VaultTransferDirection::Upload => &mut rates.upload,
                teleark_runtime::VaultTransferDirection::Download => &mut rates.download,
            };
            *total = total
                .zip(measured)
                .map(|(sum, rate)| sum.saturating_add(rate));
        }
        self.cached = Some(CachedRates {
            account,
            native: native_key,
            vault: vault_key,
            revisions,
            rates,
        });
        rates
    }
}
