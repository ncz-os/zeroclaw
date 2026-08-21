//! Trait abstraction for session persistence backends.

use chrono::{DateTime, Utc};
use zeroclaw_api::model_provider::ChatMessage;
use std::sync::Arc;
use std::path::Path;

/// Metadata about a persisted session.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// Session key (e.g. `telegram_user123`).
    pub key: String,
    /// Optional human-readable name (e.g. `eyrie-commander-briefing`).
    pub name: Option<String>,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the last message was appended.
    pub last_activity: DateTime<Utc>,
    /// Total number of messages in the session.
    pub message_count: usize,
    /// Alias of the agent that owned this session (HashMap key in
    /// `config.agents`). `None` for sessions persisted before per-agent
    /// attribution landed, or for backends that don't track it.
    pub agent_alias: Option<String>,
    /// Dotted ChannelRef the session belongs to (`<type>.<alias>`,
    /// e.g. `discord.clamps`). `None` for non-channel sessions (CLI,
    /// internal cron runs) or backends without routing columns.
    pub channel_id: Option<String>,
    /// Platform-side room / thread identifier (Discord channel id,
    /// Matrix room id, Slack thread ts, ...). `None` for direct messages
    /// or backends that don't track it.
    pub room_id: Option<String>,
    /// Inbound sender id verbatim (Discord username, phone number, ...).
    /// Not an FK — sessions can survive deletion of the upstream user.
    pub sender_id: Option<String>,
}

/// Structured routing context recorded alongside a session. Mirrors the
/// `ChannelMessage` fields the orchestrator uses to compose
/// `conversation_history_key` so the session row can be queried by
/// channel / room / sender without re-parsing the synthetic key.
#[derive(Debug, Clone, Default)]
pub struct SessionContext<'a> {
    /// `<type>.<alias>` ChannelRef (`discord.clamps`).
    pub channel_id: Option<&'a str>,
    /// Platform-side room / thread id.
    pub room_id: Option<&'a str>,
    /// Inbound sender id (channel-native username, phone, ...).
    pub sender_id: Option<&'a str>,
}

/// Owned version of SessionContext for async operations.
#[derive(Debug, Clone, Default)]
pub struct OwnedSessionContext {
    /// `<type>.<alias>` ChannelRef (`discord.clamps`).
    pub channel_id: Option<String>,
    /// Platform-side room / thread id.
    pub room_id: Option<String>,
    /// Inbound sender id (channel-native username, phone, ...).
    pub sender_id: Option<String>,
}

impl<'a> From<SessionContext<'a>> for OwnedSessionContext {
    fn from(ctx: SessionContext<'a>) -> Self {
        Self {
            channel_id: ctx.channel_id.map(|s| s.to_string()),
            room_id: ctx.room_id.map(|s| s.to_string()),
            sender_id: ctx.sender_id.map(|s| s.to_string()),
        }
    }
}

impl<'a> From<&'a OwnedSessionContext> for SessionContext<'a> {
    fn from(ctx: &'a OwnedSessionContext) -> Self {
        Self {
            channel_id: ctx.channel_id.as_deref(),
            room_id: ctx.room_id.as_deref(),
            sender_id: ctx.sender_id.as_deref(),
        }
    }
}

/// Query parameters for listing sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionQuery {
    /// Keyword to search in session messages (FTS5 if available).
    pub keyword: Option<String>,
    /// Maximum number of sessions to return.
    pub limit: Option<usize>,
}

/// One persisted message with the optional `created_at` the backend
/// stamped on it. JSONL / in-memory backends return `None`; SQLite
/// returns the row's `created_at` column.
#[derive(Debug, Clone)]
pub struct TimestampedMessage {
    pub message: ChatMessage,
    pub created_at: Option<DateTime<Utc>>,
}

/// Trait for session persistence backends.
///
/// Implementations must be `Send + Sync` for sharing across async tasks.
///
/// ## Fallible Read Contract
///
/// All read methods return `std::io::Result<T>`. A successful `Ok` result
/// means the backend read succeeded. An `Err` means a real I/O failure
/// occurred (corrupt database, permission denied, disk full, etc.).
/// Callers must NOT treat read errors as "empty data" — errors must be
/// logged and the operation failed explicitly.
///
/// ## Multiword Search Contract
///
/// The `search` method interprets multiword queries as **OR** combinations:
/// a query `"foo bar"` returns sessions containing `foo` OR `bar` (not both).
/// This preserves SQLite FTS5's default behavior so switching backends never
/// silently narrows results. Later backends MUST match this contract.
pub trait SessionBackend: Send + Sync {
    /// Load all messages for a session.
    ///
    /// Returns `Ok(Vec::new())` only when the session genuinely doesn't exist.
    /// Any I/O error (corrupt DB, permission denied, etc.) returns `Err`.
    fn load(&self, session_key: &str) -> std::io::Result<Vec<ChatMessage>>;

    /// Same as `load`, but each row carries its persisted `created_at`
    /// when the backend has one.
    fn load_with_timestamps(&self, session_key: &str) -> std::io::Result<Vec<TimestampedMessage>> {
        Ok(self
            .load(session_key)?
            .into_iter()
            .map(|message| TimestampedMessage {
                message,
                created_at: None,
            })
            .collect())
    }

    /// Append a single message to a session.
    fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()>;

    /// Remove the last message from a session. Returns `true` if a message was removed.
    fn remove_last(&self, session_key: &str) -> std::io::Result<bool>;

    fn update_last(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<bool> {
        if self.remove_last(session_key)? {
            self.append(session_key, message)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all session keys.
    ///
    /// Returns `Ok(Vec::new())` only when no sessions exist.
    /// Any I/O error (unreadable directory, corrupt DB, etc.) returns `Err`.
    fn list_sessions(&self) -> std::io::Result<Vec<String>>;

    /// List sessions with metadata.
    ///
    /// ## Performance Note
    ///
    /// The default implementation calls `load()` and `get_session_name()` for each
    /// session, resulting in O(N) database queries. Implementations with a joined
    /// query capability (e.g., SQLite) should override this for better performance.
    fn list_sessions_with_metadata(&self) -> std::io::Result<Vec<SessionMetadata>> {
        let sessions = self.list_sessions()?;
        sessions
            .into_iter()
            .map(|key| -> std::io::Result<_> {
                let messages = self.load(&key)?;
                let name = self.get_session_name(&key).ok().flatten();
                Ok(SessionMetadata {
                    key,
                    name,
                    created_at: Utc::now(),
                    last_activity: Utc::now(),
                    message_count: messages.len(),
                    agent_alias: None,
                    channel_id: None,
                    room_id: None,
                    sender_id: None,
                })
            })
            .collect::<std::io::Result<Vec<_>>>()
    }

    /// Compact a session file (remove duplicates/corruption). No-op by default.
    fn compact(&self, _session_key: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Remove sessions that haven't been active within the given TTL hours.
    fn cleanup_stale(&self, _ttl_hours: u32) -> std::io::Result<usize> {
        Ok(0)
    }

    /// Search sessions by keyword.
    ///
    /// Returns `Ok(Vec::new())` when no sessions match the query.
    /// Any I/O error returns `Err`.
    fn search(&self, query: &SessionQuery) -> std::io::Result<Vec<SessionMetadata>> {
        let _ = query;
        Ok(Vec::new())
    }

    fn clear_messages(&self, session_key: &str) -> std::io::Result<usize> {
        let mut count = 0;
        while self.remove_last(session_key)? {
            count += 1;
        }
        Ok(count)
    }

    /// Delete all messages for a session. Returns `true` if the session existed.
    fn delete_session(&self, _session_key: &str) -> std::io::Result<bool> {
        Ok(false)
    }

    fn clear_agent_attribution(&self, _agent_alias: &str) -> std::io::Result<usize> {
        Ok(0)
    }

    fn rename_agent_attribution(&self, _from: &str, _to: &str) -> std::io::Result<usize> {
        Ok(0)
    }

    fn count_agent_attribution(&self, _agent_alias: &str) -> std::io::Result<usize> {
        Ok(0)
    }

    fn session_exists(&self, session_key: &str) -> std::io::Result<bool> {
        match self.get_session_metadata(session_key) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Set or update the human-readable name for a session.
    fn set_session_name(&self, _session_key: &str, _name: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Get the human-readable name for a session (if set).
    fn get_session_name(&self, _session_key: &str) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    /// Record the agent alias that owns a session. Called on WebSocket
    /// handshake when the alias is known. No-op for backends that don't
    /// track per-agent attribution.
    fn set_session_agent_alias(
        &self,
        _session_key: &str,
        _agent_alias: &str,
    ) -> std::io::Result<()> {
        Ok(())
    }

    /// Get the agent alias associated with a session, if recorded.
    fn get_session_agent_alias(&self, _session_key: &str) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    fn set_session_context(
        &self,
        _session_key: &str,
        _context: SessionContext<'_>,
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn get_session_metadata(&self, session_key: &str) -> std::io::Result<Option<SessionMetadata>> {
        let messages = self.load(session_key)?;
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(SessionMetadata {
            key: session_key.to_string(),
            name: self.get_session_name(session_key).ok().flatten(),
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: messages.len(),
            agent_alias: None,
            channel_id: None,
            room_id: None,
            sender_id: None,
        }))
    }

    /// Set the session state (e.g. "idle", "running", "error").
    /// `turn_id` identifies the current turn (set when running, cleared on idle).
    fn set_session_state(
        &self,
        _session_key: &str,
        _state: &str,
        _turn_id: Option<&str>,
    ) -> std::io::Result<()> {
        Ok(())
    }

    /// Get the current session state. Returns `None` if the backend doesn't track state.
    fn get_session_state(&self, _session_key: &str) -> std::io::Result<Option<SessionState>> {
        Ok(None)
    }

    /// List sessions currently in "running" state.
    fn list_running_sessions(&self) -> std::io::Result<Vec<SessionMetadata>> {
        Ok(Vec::new())
    }

    /// List sessions stuck in "running" state longer than `threshold_secs`.
    fn list_stuck_sessions(&self, _threshold_secs: u64) -> std::io::Result<Vec<SessionMetadata>> {
        Ok(Vec::new())
    }
}

/// Shared async facade for SessionBackend operations.
///
/// This facade wraps the synchronous SessionBackend trait and executes all
/// operations on a blocking task pool, preventing blocking calls from stalling
/// async executors. This is the SINGLE shared facade used by all async consumers
/// (WS/RPC/channel paths), replacing per-caller blocking adapters.
///
/// ## Usage
///
/// ```rust,ignore
/// let backend: Arc<dyn SessionBackend> = /* ... */;
/// let async_backend = AsyncSessionBackend::new(backend);
///
/// // Use in async contexts without blocking the executor
/// let messages = async_backend.load("session_key").await?;
/// ```
///
/// ## F1 Compliance
///
/// - ONE shared facade per process (injected at bootstrap)
/// - NO per-caller adapters (gateway, runtime, tools all use this)
/// - Backend construction happens once at bootstrap, off async entry paths
pub struct AsyncSessionBackend {
    /// The underlying synchronous backend (public for testing)
    pub backend: Arc<dyn SessionBackend>,
}

impl AsyncSessionBackend {
    /// Create a new async facade wrapping a synchronous backend.
    pub fn new(backend: Arc<dyn SessionBackend>) -> Self {
        Self { backend }
    }

    /// Load all messages for a session (async).
    pub async fn load(&self, session_key: &str) -> std::io::Result<Vec<ChatMessage>> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.load(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Load messages with timestamps (async).
    pub async fn load_with_timestamps(
        &self,
        session_key: &str,
    ) -> std::io::Result<Vec<TimestampedMessage>> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.load_with_timestamps(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Append a message to a session (async).
    pub async fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        let message = message.clone();
        tokio::task::spawn_blocking(move || backend.append(&session_key, &message))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// List all session keys (async).
    pub async fn list_sessions(&self) -> std::io::Result<Vec<String>> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || backend.list_sessions())
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// List sessions with metadata (async).
    pub async fn list_sessions_with_metadata(&self) -> std::io::Result<Vec<SessionMetadata>> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || backend.list_sessions_with_metadata())
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Check if a session exists (async).
    pub async fn session_exists(&self, session_key: &str) -> std::io::Result<bool> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.session_exists(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Get session metadata (async).
    pub async fn get_session_metadata(&self, session_key: &str) -> std::io::Result<Option<SessionMetadata>> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.get_session_metadata(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Set session name (async).
    pub async fn set_session_name(&self, session_key: &str, name: &str) -> std::io::Result<()> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || backend.set_session_name(&session_key, &name))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Get session name (async).
    pub async fn get_session_name(&self, session_key: &str) -> std::io::Result<Option<String>> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.get_session_name(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Set session agent alias (async).
    pub async fn set_session_agent_alias(&self, session_key: &str, agent_alias: &str) -> std::io::Result<()> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        let agent_alias = agent_alias.to_string();
        tokio::task::spawn_blocking(move || backend.set_session_agent_alias(&session_key, &agent_alias))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Get session agent alias (async).
    pub async fn get_session_agent_alias(&self, session_key: &str) -> std::io::Result<Option<String>> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.get_session_agent_alias(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Set session context (async).
    pub async fn set_session_context(&self, session_key: &str, context: OwnedSessionContext) -> std::io::Result<()> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || {
            backend.set_session_context(&session_key, SessionContext {
                channel_id: context.channel_id.as_deref(),
                room_id: context.room_id.as_deref(),
                sender_id: context.sender_id.as_deref(),
            })
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
        .and_then(|result| result)
    }

    /// Set session state (async).
    pub async fn set_session_state(&self, session_key: &str, state: &str, turn_id: Option<&str>) -> std::io::Result<()> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        let state = state.to_string();
        let turn_id = turn_id.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            backend.set_session_state(&session_key, &state, turn_id.as_deref())
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
        .and_then(|result| result)
    }

    /// Get session state (async).
    pub async fn get_session_state(&self, session_key: &str) -> std::io::Result<Option<SessionState>> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.get_session_state(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result.map(|opt| opt.map(|s| SessionState {
                state: s.state,
                turn_id: s.turn_id,
                turn_started_at: s.turn_started_at,
            })))
    }

    /// List running sessions (async).
    pub async fn list_running_sessions(&self) -> std::io::Result<Vec<SessionMetadata>> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || backend.list_running_sessions())
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// List stuck sessions (async).
    pub async fn list_stuck_sessions(&self, threshold_secs: u64) -> std::io::Result<Vec<SessionMetadata>> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || backend.list_stuck_sessions(threshold_secs))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Remove last message from a session (async).
    pub async fn remove_last(&self, session_key: &str) -> std::io::Result<bool> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.remove_last(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Update last message in a session (async).
    pub async fn update_last(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<bool> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        let message = message.clone();
        tokio::task::spawn_blocking(move || backend.update_last(&session_key, &message))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Clear all messages from a session (async).
    pub async fn clear_messages(&self, session_key: &str) -> std::io::Result<usize> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.clear_messages(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Delete a session (async).
    pub async fn delete_session(&self, session_key: &str) -> std::io::Result<bool> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.delete_session(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Search sessions by keyword (async).
    pub async fn search(&self, query: &SessionQuery) -> std::io::Result<Vec<SessionMetadata>> {
        let backend = self.backend.clone();
        let query = query.clone();
        tokio::task::spawn_blocking(move || backend.search(&query))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Compact a session file (async).
    pub async fn compact(&self, session_key: &str) -> std::io::Result<()> {
        let backend = self.backend.clone();
        let session_key = session_key.to_string();
        tokio::task::spawn_blocking(move || backend.compact(&session_key))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Cleanup stale sessions (async).
    pub async fn cleanup_stale(&self, ttl_hours: u32) -> std::io::Result<usize> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || backend.cleanup_stale(ttl_hours))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Rename agent attribution (async).
    pub async fn rename_agent_attribution(&self, from: &str, to: &str) -> std::io::Result<usize> {
        let backend = self.backend.clone();
        let from = from.to_string();
        let to = to.to_string();
        tokio::task::spawn_blocking(move || backend.rename_agent_attribution(&from, &to))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Clear agent attribution (async).
    pub async fn clear_agent_attribution(&self, agent_alias: &str) -> std::io::Result<usize> {
        let backend = self.backend.clone();
        let agent_alias = agent_alias.to_string();
        tokio::task::spawn_blocking(move || backend.clear_agent_attribution(&agent_alias))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }

    /// Count agent attribution (async).
    pub async fn count_agent_attribution(&self, agent_alias: &str) -> std::io::Result<usize> {
        let backend = self.backend.clone();
        let agent_alias = agent_alias.to_string();
        tokio::task::spawn_blocking(move || backend.count_agent_attribution(&agent_alias))
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Task join error: {e}")))
            .and_then(|result| result)
    }
}

/// Session state information.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Current state: "idle", "running", or "error".
    pub state: String,
    /// Turn ID of the active or last turn.
    pub turn_id: Option<String>,
    /// When the current state was entered.
    pub turn_started_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_metadata_is_constructible() {
        let meta = SessionMetadata {
            key: "test".into(),
            name: None,
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 5,
            agent_alias: None,
            channel_id: None,
            room_id: None,
            sender_id: None,
        };
        assert_eq!(meta.key, "test");
        assert_eq!(meta.message_count, 5);
    }

    #[test]
    fn session_query_defaults() {
        let q = SessionQuery::default();
        assert!(q.keyword.is_none());
        assert!(q.limit.is_none());
    }
}
