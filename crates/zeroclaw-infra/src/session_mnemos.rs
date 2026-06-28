//! MNEMOS-backed session persistence (the ncz-os "MNEMOS-first" backend).
//!
//! Stores each conversation turn as a memory in a MNEMOS datastore via its REST
//! API, so a fleet of headless zoder/zeroclaw workers shares one durable system
//! of record for session history — and that history lives alongside the rest of
//! the agent's MNEMOS memory instead of in scattered local files.
//!
//! # Feature flag
//!
//! Requires `--features backend-mnemos` (adds a blocking `ureq` HTTP client).
//!
//! # Configuration
//!
//! ```toml
//! [channels]
//! session_backend = "mnemos"
//! mnemos_url = "http://mnemos-host:5002"
//! # token read from the MNEMOS_TOKEN env var by the factory (never inlined)
//! mnemos_category = "zoder-session"   # optional; defaults to "zoder-session"
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
use chrono::Utc;
use serde_json::{Value, json};
use std::io;
use zeroclaw_api::model_provider::ChatMessage;

const DEFAULT_CATEGORY: &str = "zoder-session";
/// Upper bound on messages fetched per session / sessions listed. Sessions far
/// larger than this should use a SQL backend; MNEMOS is the shared-memory tier.
const FETCH_LIMIT: usize = 10_000;

/// MNEMOS REST session backend (blocking `ureq`, fits the synchronous
/// [`SessionBackend`] contract; async callers wrap it in `spawn_blocking`).
pub struct MnemosSessionBackend {
    base_url: String,
    token: Option<String>,
    category: String,
    agent: ureq::Agent,
}

impl MnemosSessionBackend {
    /// `base_url` is the MNEMOS API root (e.g. `http://host:5002`); `token` is an
    /// optional bearer token; `category` groups this fleet's session memories
    /// (defaults to `zoder-session` when `None`/empty).
    pub fn new(base_url: &str, token: Option<String>, category: Option<&str>) -> Self {
        let category = category
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or(DEFAULT_CATEGORY)
            .to_string();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.is_empty()),
            category,
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(15))
                .build(),
        }
    }

    fn auth(&self, req: ureq::Request) -> ureq::Request {
        match &self.token {
            Some(t) => req.set("Authorization", &format!("Bearer {t}")),
            None => req,
        }
    }

    /// GET memories for a session (or, with `key = None`, every session memory
    /// under this category). Read errors degrade to an empty list — a transient
    /// MNEMOS blip must not crash the agent; the in-memory history still serves
    /// the live turn.
    fn fetch(&self, key: Option<&str>) -> Vec<Value> {
        let mut url = format!(
            "{}/v1/memories?category={}&limit={FETCH_LIMIT}",
            self.base_url,
            urlencode(&self.category),
        );
        if let Some(k) = key {
            url.push_str(&format!("&subcategory={}", urlencode(k)));
        }
        let resp = match self.auth(self.agent.get(&url)).call() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let body: Value = match resp.into_json() {
            Ok(v) => v,
            Err(_) => return Vec::new(),
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
            self.agent
                .delete(&format!("{}/v1/memories/{}", self.base_url, urlencode(id))),
        )
        .call()
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
        self.auth(self.agent.post(&format!("{}/v1/memories", self.base_url)))
            .send_json(body)
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
        Some(SessionMetadata {
            key: session_key.to_string(),
            name: None,
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: items.len(),
            agent_alias: None,
            channel_id: None,
            room_id: None,
            sender_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let b = MnemosSessionBackend::new("http://h:5002/", Some(String::new()), Some("  "));
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
}
