//! MNEMOS-backed session persistence (the ncz-os "MNEMOS-first" backend).
//!
//! Stores each conversation turn as a memory in a MNEMOS datastore via its REST
//! API, so a fleet of headless zoder/zeroclaw workers shares one durable system
//! of record for session history — and that history lives alongside the rest of
//! the agent's MNEMOS memory instead of in scattered local files.
//!
//! # Feature flag
//!
//! Requires `--features backend-mnemos` (adds a blocking `reqwest` client).
//!
//! # Configuration
//!
//! ```toml
//! [channels]
//! session_backend = "mnemos"
//! mnemos_url = "http://mnemos-host:5002"
//! # bearer token comes from the MNEMOS_TOKEN env var, read by the factory in
//! # lib.rs — it is deliberately never read from (or written to) the config file
//! mnemos_category = "zoder-session"   # optional; defaults to "zoder-session"
//! mnemos_pool_size = 32               # optional; idle connections kept per host
//! ```
//!
//! # Data model
//!
//! Each message → one memory: `content` is the message text, `category` is the
//! configured session category (default `zoder-session`), `subcategory` is the
//! session key, and `metadata.role` carries the chat role. Retrieval filters
//! server-side on `?category=&subcategory=` (verified against MNEMOS v6) and
//! orders by the memory id (`mem_<epoch_ms>_<hash>`), which is monotonic in
//! creation time at millisecond granularity. The orchestrator serializes
//! per-session appends (#7753), so creation order is the conversation order.

use crate::session_backend::{SessionBackend, SessionMetadata};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use std::io;
use zeroclaw_api::model_provider::ChatMessage;

const DEFAULT_CATEGORY: &str = "zoder-session";
/// Upper bound on messages fetched per session / sessions listed. Sessions far
/// larger than this should use a SQL backend; MNEMOS is the shared-memory tier.
const FETCH_LIMIT: usize = 10_000;

/// MNEMOS REST session backend (blocking `reqwest`, fits the synchronous
/// [`SessionBackend`] contract; async callers wrap it in `spawn_blocking`).
pub struct MnemosSessionBackend {
    base_url: String,
    token: Option<String>,
    category: String,
    client: reqwest::blocking::Client,
}

impl MnemosSessionBackend {
    /// `base_url` is the MNEMOS API root (e.g. `http://host:5002`); `token` is an
    /// optional bearer token; `category` groups this fleet's session memories
    /// (defaults to `zoder-session` when `None`/empty); `pool_size` caps idle
    /// connections kept per host.
    pub fn new(
        base_url: &str,
        token: Option<String>,
        category: Option<&str>,
        pool_size: u16,
    ) -> Self {
        let category = category
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or(DEFAULT_CATEGORY)
            .to_string();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .connect_timeout(std::time::Duration::from_secs(5))
            .pool_max_idle_per_host(pool_size as usize)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.is_empty()),
            category,
            client,
        }
    }

    fn auth(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// GET memories for a session (or, with `key = None`, every session memory
    /// under this category). Read errors degrade to an empty list — a transient
    /// MNEMOS blip must not crash the agent; the in-memory history still serves
    /// the live turn. The failure is logged rather than swallowed silently, so
    /// an auth or connectivity fault is not mistaken for an empty session.
    fn fetch(&self, key: Option<&str>) -> Vec<Value> {
        let mut url = format!(
            "{}/v1/memories?category={}&limit={FETCH_LIMIT}",
            self.base_url,
            urlencode(&self.category),
        );
        if let Some(k) = key {
            url.push_str(&format!("&subcategory={}", urlencode(k)));
        }
        // `error_for_status` matters here: reqwest treats 4xx/5xx as success at
        // the transport layer, so without it an unauthenticated MNEMOS would
        // parse as "no memories" instead of surfacing the 401.
        let result = self
            .auth(self.client.get(&url))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::json::<Value>);
        let body = match result {
            Ok(v) => v,
            Err(e) => {
                ::zeroclaw_log::record!(
                    WARN,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                        .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                        .with_attrs(::serde_json::json!({"error": e.to_string()})),
                    "MNEMOS session read failed; treating this session as empty for now"
                );
                return Vec::new();
            }
        };
        body.get("memories")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Memory id, used both as the stable delete handle and the creation-order
    /// sort key (`mem_<epoch_ms>_<hash>` sorts chronologically).
    fn id_of(item: &Value) -> Option<String> {
        item.get("id").and_then(Value::as_str).map(str::to_string)
    }

    /// Recover the creation time a memory id encodes (`mem_<epoch_ms>_<hash>`).
    fn created_at_of(item: &Value) -> Option<DateTime<Utc>> {
        let id = Self::id_of(item)?;
        let ms: i64 = id.strip_prefix("mem_")?.split('_').next()?.parse().ok()?;
        Utc.timestamp_millis_opt(ms).single()
    }

    fn sorted_for_session(&self, key: &str) -> Vec<Value> {
        let mut items = self.fetch(Some(key));
        items.sort_by(|a, b| {
            Self::id_of(a)
                .unwrap_or_default()
                .cmp(&Self::id_of(b).unwrap_or_default())
        });
        items
    }

    fn delete_id(&self, id: &str) -> io::Result<()> {
        self.auth(
            self.client
                .delete(format!("{}/v1/memories/{}", self.base_url, urlencode(id))),
        )
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map(|_| ())
        .map_err(|e| io::Error::other(format!("mnemos delete {id}: {e}")))
    }
}

fn urlencode(s: &str) -> String {
    // Percent-encode the characters that matter for a path/query segment.
    // Session keys are channel/room ids and uuids — conservative is fine.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn item_to_message(item: &Value) -> ChatMessage {
    let content = item
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let role = item
        .get("metadata")
        .and_then(|m| m.get("role"))
        .and_then(Value::as_str)
        .unwrap_or("user")
        .to_string();
    ChatMessage { role, content }
}

impl SessionBackend for MnemosSessionBackend {
    fn load(&self, session_key: &str) -> Vec<ChatMessage> {
        self.sorted_for_session(session_key)
            .iter()
            .map(item_to_message)
            .collect()
    }

    fn append(&self, session_key: &str, message: &ChatMessage) -> io::Result<()> {
        let body = json!({
            "content": message.content,
            "category": self.category,
            "subcategory": session_key,
            "source_session": session_key,
            "metadata": { "role": message.role },
        });
        self.auth(self.client.post(format!("{}/v1/memories", self.base_url)))
            .json(&body)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map(|_| ())
            .map_err(|e| io::Error::other(format!("mnemos append to {session_key}: {e}")))
    }

    fn remove_last(&self, session_key: &str) -> io::Result<bool> {
        let items = self.sorted_for_session(session_key);
        match items.last().and_then(Self::id_of) {
            Some(id) => {
                self.delete_id(&id)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn list_sessions(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .fetch(None)
            .iter()
            .filter_map(|m| {
                m.get("subcategory")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    fn session_exists(&self, session_key: &str) -> bool {
        !self.fetch(Some(session_key)).is_empty()
    }

    fn delete_session(&self, session_key: &str) -> io::Result<bool> {
        let ids: Vec<String> = self
            .fetch(Some(session_key))
            .iter()
            .filter_map(Self::id_of)
            .collect();
        if ids.is_empty() {
            return Ok(false);
        }
        for id in &ids {
            self.delete_id(id)?;
        }
        Ok(true)
    }

    fn clear_messages(&self, session_key: &str) -> io::Result<usize> {
        let ids: Vec<String> = self
            .fetch(Some(session_key))
            .iter()
            .filter_map(Self::id_of)
            .collect();
        let n = ids.len();
        for id in &ids {
            self.delete_id(id)?;
        }
        Ok(n)
    }

    fn get_session_metadata(&self, session_key: &str) -> Option<SessionMetadata> {
        let items = self.sorted_for_session(session_key);
        if items.is_empty() {
            return None;
        }
        // Items are sorted by id, which encodes creation time, so the ends of
        // the list give the real session bounds. Fall back to "now" only when a
        // malformed id makes the timestamp unrecoverable.
        let created_at = items.first().and_then(Self::created_at_of);
        let last_activity = items.last().and_then(Self::created_at_of);
        Some(SessionMetadata {
            key: session_key.to_string(),
            name: None,
            created_at: created_at.unwrap_or_else(Utc::now),
            last_activity: last_activity.or(created_at).unwrap_or_else(Utc::now),
            message_count: items.len(),
            agent_alias: None,
            channel_id: None,
            room_id: None,
            sender_id: None,
        })
    }

    fn session_backend_name(&self) -> &'static str {
        "mnemos"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> MnemosSessionBackend {
        MnemosSessionBackend::new("http://h:5002", None, None, 32)
    }

    #[test]
    fn urlencode_preserves_unreserved_and_escapes_the_rest() {
        assert_eq!(urlencode("abcDEF-123_.~"), "abcDEF-123_.~");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
        assert_eq!(urlencode("room:42#x"), "room%3A42%23x");
    }

    #[test]
    fn item_to_message_reads_content_and_metadata_role() {
        let item = json!({"content": "hi", "metadata": {"role": "assistant"}});
        let m = item_to_message(&item);
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content, "hi");
    }

    #[test]
    fn item_to_message_defaults_role_to_user() {
        let item = json!({"content": "hi"});
        assert_eq!(item_to_message(&item).role, "user");
    }

    #[test]
    fn new_defaults_blank_category_and_strips_trailing_slash() {
        let b = MnemosSessionBackend::new("http://h:5002/", Some(String::new()), Some("  "), 32);
        assert_eq!(b.base_url, "http://h:5002");
        assert_eq!(b.category, DEFAULT_CATEGORY);
        assert!(b.token.is_none(), "empty token should be dropped");
    }

    #[test]
    fn id_ordering_is_chronological() {
        // mem_<epoch_ms>_<hash> sorts by the embedded timestamp.
        let mut v = [
            json!({"id": "mem_1782675392144_b"}),
            json!({"id": "mem_1782675368707_a"}),
        ];
        v.sort_by(|a, b| {
            MnemosSessionBackend::id_of(a)
                .unwrap()
                .cmp(&MnemosSessionBackend::id_of(b).unwrap())
        });
        assert_eq!(
            MnemosSessionBackend::id_of(&v[0]).unwrap(),
            "mem_1782675368707_a"
        );
    }

    #[test]
    fn created_at_is_recovered_from_the_memory_id() {
        let item = json!({"id": "mem_1782675368707_a"});
        let ts = MnemosSessionBackend::created_at_of(&item).expect("timestamp parses");
        assert_eq!(ts.timestamp_millis(), 1782675368707);
    }

    #[test]
    fn created_at_of_rejects_malformed_ids() {
        // Guards the unwrap_or_else(Utc::now) fallback in get_session_metadata.
        for bad in ["", "mem_", "mem_notanumber_x", "1782675368707", "xxx_1_2"] {
            let item = json!({ "id": bad });
            assert!(
                MnemosSessionBackend::created_at_of(&item).is_none(),
                "expected {bad:?} to yield no timestamp"
            );
        }
    }

    #[test]
    fn pool_size_is_accepted_and_backend_names_itself() {
        // pool_size is plumbed into the client builder; assert the knob is at
        // least wired far enough to construct a working backend.
        let b = backend();
        assert_eq!(b.session_backend_name(), "mnemos");
    }
}
