use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use grammers_session::{
    BoxFuture, Session, SessionData,
    types::{ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState},
};
use serde::{Deserialize, Serialize};

const SESSION_FORMAT_VERSION: u32 = 1;
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum FileSessionError {
    Io(io::Error),
    Format,
    Poisoned,
}

impl fmt::Display for FileSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("Telegram session storage I/O failure"),
            Self::Format => formatter.write_str("Telegram session storage format failure"),
            Self::Poisoned => formatter.write_str("Telegram session storage lock failure"),
        }
    }
}

impl Error for FileSessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Format | Self::Poisoned => None,
        }
    }
}

impl From<io::Error> for FileSessionError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Serialize, Deserialize)]
struct EncodedSession {
    version: u32,
    home_dc: i32,
    dc_options: Vec<DcOption>,
    peer_infos: Vec<PeerInfo>,
    updates_state: UpdatesState,
}

impl From<SessionData> for EncodedSession {
    fn from(value: SessionData) -> Self {
        Self {
            version: SESSION_FORMAT_VERSION,
            home_dc: value.home_dc,
            dc_options: value.dc_options.into_values().collect(),
            peer_infos: value.peer_infos.into_values().collect(),
            updates_state: value.updates_state,
        }
    }
}

impl TryFrom<EncodedSession> for SessionData {
    type Error = FileSessionError;

    fn try_from(value: EncodedSession) -> Result<Self, Self::Error> {
        if value.version != SESSION_FORMAT_VERSION {
            return Err(FileSessionError::Format);
        }
        Ok(Self {
            home_dc: value.home_dc,
            dc_options: value
                .dc_options
                .into_iter()
                .map(|option| (option.id, option))
                .collect(),
            peer_infos: value
                .peer_infos
                .into_iter()
                .map(|peer| (peer.id(), peer))
                .collect(),
            updates_state: value.updates_state,
        })
    }
}

pub(crate) struct FileSession {
    path: PathBuf,
    data: Mutex<SessionData>,
}

impl FileSession {
    pub(crate) fn clear_authorization(&self) -> Result<(), FileSessionError> {
        self.mutate(|data| *data = SessionData::default())
    }

    pub(crate) fn open(path: &Path) -> Result<Self, FileSessionError> {
        let data = match fs::metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.len() > MAX_SESSION_BYTES {
                    return Err(FileSessionError::Format);
                }
                let bytes = fs::read(path)?;
                serde_json::from_slice::<EncodedSession>(&bytes)
                    .map_err(|_| FileSessionError::Format)?
                    .try_into()?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => SessionData::default(),
            Err(error) => return Err(FileSessionError::Io(error)),
        };
        let session = Self {
            path: path.to_owned(),
            data: Mutex::new(data),
        };
        session.persist()?;
        Ok(session)
    }

    fn data(&self) -> Result<MutexGuard<'_, SessionData>, FileSessionError> {
        self.data.lock().map_err(|_| FileSessionError::Poisoned)
    }

    fn persist(&self) -> Result<(), FileSessionError> {
        let data = self.data()?;
        let encoded = EncodedSession {
            version: SESSION_FORMAT_VERSION,
            home_dc: data.home_dc,
            dc_options: data.dc_options.values().cloned().collect(),
            peer_infos: data.peer_infos.values().cloned().collect(),
            updates_state: data.updates_state.clone(),
        };
        let bytes = serde_json::to_vec(&encoded).map_err(|_| FileSessionError::Format)?;
        if bytes.len() as u64 > MAX_SESSION_BYTES {
            return Err(FileSessionError::Format);
        }
        let temporary = self.path.with_extension("session.tmp");
        fs::write(&temporary, bytes)?;
        restrict_permissions(&temporary)?;
        fs::rename(&temporary, &self.path)?;
        restrict_permissions(&self.path)?;
        Ok(())
    }

    fn mutate(&self, operation: impl FnOnce(&mut SessionData)) -> Result<(), FileSessionError> {
        {
            let mut data = self.data()?;
            operation(&mut data);
        }
        self.persist()
    }
}

impl Session for FileSession {
    type Error = FileSessionError;

    fn home_dc_id(&self) -> Result<i32, Self::Error> {
        Ok(self.data()?.home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Self::Error>> {
        Box::pin(async move { self.mutate(|data| data.home_dc = dc_id) })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Self::Error> {
        Ok(self.data()?.dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Self::Error>> {
        let option = dc_option.clone();
        Box::pin(async move {
            self.mutate(|data| {
                data.dc_options.insert(option.id, option);
            })
        })
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Self::Error>> {
        Box::pin(async move { Ok(self.data()?.peer_infos.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: &PeerInfo) -> BoxFuture<'_, Result<(), Self::Error>> {
        let peer = peer.clone();
        Box::pin(async move {
            self.mutate(|data| {
                data.peer_infos
                    .entry(peer.id())
                    .and_modify(|stored| {
                        stored.extend_info(&peer);
                    })
                    .or_insert(peer);
            })
        })
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Self::Error>> {
        Box::pin(async move { Ok(self.data()?.updates_state.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Self::Error>> {
        Box::pin(async move {
            self.mutate(|data| match update {
                UpdateState::All(state) => data.updates_state = state,
                UpdateState::Primary { pts, date, seq } => {
                    data.updates_state.pts = pts;
                    data.updates_state.date = date;
                    data.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => data.updates_state.qts = qts,
                UpdateState::Channel { id, pts } => {
                    data.updates_state
                        .channels
                        .retain(|channel| channel.id != id);
                    data.updates_state.channels.push(ChannelState { id, pts });
                }
            })
        })
    }
}

fn restrict_permissions(path: &Path) -> Result<(), FileSessionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip_and_permissions() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("account.session");
        let session = FileSession::open(&path)?;
        tokio::runtime::Builder::new_current_thread()
            .build()?
            .block_on(session.set_home_dc_id(4))?;
        drop(session);
        let reopened = FileSession::open(&path)?;
        assert_eq!(reopened.home_dc_id()?, 4);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn oversized_or_unknown_session_is_rejected() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("account.session");
        fs::write(&path, br#"{"version":999}"#)?;
        assert!(matches!(
            FileSession::open(&path),
            Err(FileSessionError::Format)
        ));
        assert!(crate::TelegramConnection::clear_revoked_session(&path).is_err());
        assert_eq!(fs::read(&path)?, br#"{"version":999}"#);
        Ok(())
    }

    #[test]
    fn revoked_session_reset_is_restartable_and_failed_save_preserves_original()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("account.session");
        let session = FileSession::open(&path)?;
        session.mutate(|data| {
            data.home_dc = 4;
            for dc in data.dc_options.values_mut() {
                dc.auth_key = Some([7; 256]); // Synthetic key, never a real session.
            }
        })?;
        let original = fs::read(&path)?;
        // Deterministic failed atomic replacement, with no permissions assumptions.
        let temporary = path.with_extension("session.tmp");
        fs::create_dir(&temporary)?;
        assert!(session.clear_authorization().is_err());
        assert_eq!(fs::read(&path)?, original);
        fs::remove_dir(&temporary)?;
        drop(session);
        crate::TelegramConnection::clear_revoked_session(&path)?;
        let reopened = FileSession::open(&path)?;
        assert_eq!(reopened.home_dc_id()?, SessionData::default().home_dc);
        assert!(reopened.data()?.peer_infos.is_empty());
        assert!(
            reopened
                .data()?
                .dc_options
                .values()
                .all(|dc| dc.auth_key.is_none())
        );
        crate::TelegramConnection::clear_revoked_session(&path)?;
        let repeated = FileSession::open(&path)?;
        assert_eq!(repeated.home_dc_id()?, reopened.home_dc_id()?);
        assert!(repeated.data()?.peer_infos.is_empty());
        assert!(
            repeated
                .data()?
                .dc_options
                .values()
                .all(|dc| dc.auth_key.is_none())
        );
        assert_eq!(
            repeated.data()?.updates_state,
            SessionData::default().updates_state
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }
}
