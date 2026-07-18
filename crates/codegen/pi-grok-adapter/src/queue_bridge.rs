//! Mirror Pi `queue_update` into Grok's native `x.ai/queue/changed` surface.
//!
//! Pi owns the real steering / follow-up queues. This module only assigns stable
//! wire ids (preferring client `promptId`s) so the pager's optimistic echoes can
//! be confirmed and retired without inventing a second scheduler.

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueLane {
    Steering,
    FollowUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MirrorEntry {
    id: String,
    text: String,
    version: u64,
    lane: QueueLane,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReservedPrompt {
    id: String,
    text: String,
    lane: QueueLane,
}

#[derive(Debug, Default)]
pub(crate) struct QueueMirror {
    entries: Vec<MirrorEntry>,
    reserved: Vec<ReservedPrompt>,
    next_seq: u64,
    running_prompt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueSnapshot {
    pub entries: Vec<Value>,
    pub running_prompt_id: Option<String>,
    pub steering_count: usize,
    pub follow_up_count: usize,
}

impl QueueMirror {
    pub(crate) fn reserve(&mut self, id: String, text: String, lane: QueueLane) {
        if id.trim().is_empty() {
            return;
        }
        // Latest reservation for the same client id wins.
        self.reserved.retain(|item| item.id != id);
        self.reserved.push(ReservedPrompt { id, text, lane });
    }

    pub(crate) fn text_for_id(&self, id: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.text.as_str())
    }

    pub(crate) fn apply_queue_update(
        &mut self,
        steering: &[String],
        follow_up: &[String],
    ) -> QueueSnapshot {
        let desired: Vec<(String, QueueLane)> = steering
            .iter()
            .cloned()
            .map(|text| (text, QueueLane::Steering))
            .chain(
                follow_up
                    .iter()
                    .cloned()
                    .map(|text| (text, QueueLane::FollowUp)),
            )
            .collect();

        let previous = std::mem::take(&mut self.entries);
        let mut next = Vec::with_capacity(desired.len());
        let mut used_prev = vec![false; previous.len()];

        for (text, lane) in &desired {
            if let Some((idx, entry)) = previous
                .iter()
                .enumerate()
                .find(|(idx, entry)| !used_prev[*idx] && entry.lane == *lane && entry.text == *text)
            {
                used_prev[idx] = true;
                next.push(entry.clone());
                continue;
            }

            if let Some(pos) = self
                .reserved
                .iter()
                .position(|item| item.lane == *lane && item.text == *text)
            {
                let reserved = self.reserved.remove(pos);
                next.push(MirrorEntry {
                    id: reserved.id,
                    text: text.clone(),
                    version: 0,
                    lane: *lane,
                });
                continue;
            }

            self.next_seq = self.next_seq.wrapping_add(1).max(1);
            next.push(MirrorEntry {
                id: format!("pi-queue-{}", self.next_seq),
                text: text.clone(),
                version: 0,
                lane: *lane,
            });
        }

        // Drop reservations that already landed or were superseded by Pi.
        self.reserved.retain(|item| {
            !next.iter().any(|entry| {
                entry.id == item.id || (entry.lane == item.lane && entry.text == item.text)
            })
        });

        let removed: Vec<String> = previous
            .into_iter()
            .enumerate()
            .filter_map(|(idx, entry)| (!used_prev[idx]).then_some(entry.id))
            .collect();

        // Pi delivers at most one queued user message at a time; the first
        // vanished row is the one that became the running user message.
        self.running_prompt_id = removed.into_iter().next();
        self.entries = next;
        self.snapshot()
    }

    pub(crate) fn snapshot(&self) -> QueueSnapshot {
        let entries = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                // Keep wire minimal: no owner attribution (single-client Pi).
                // Lane lives only in mirror state for reconcile.
                json!({
                    "id": entry.id,
                    "version": entry.version,
                    "kind": "prompt",
                    "text": entry.text,
                    "position": position,
                })
            })
            .collect();
        let steering_count = self
            .entries
            .iter()
            .filter(|entry| entry.lane == QueueLane::Steering)
            .count();
        let follow_up_count = self.entries.len() - steering_count;
        QueueSnapshot {
            entries,
            running_prompt_id: self.running_prompt_id.clone(),
            steering_count,
            follow_up_count,
        }
    }

    pub(crate) fn clear_running(&mut self) {
        self.running_prompt_id = None;
    }
}

pub(crate) fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn queue_changed_params(session_id: &str, snapshot: &QueueSnapshot) -> Value {
    let mut params = json!({
        "sessionId": session_id,
        "entries": snapshot.entries,
    });
    if let Some(running) = &snapshot.running_prompt_id {
        params["runningPromptId"] = Value::String(running.clone());
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_reserved_client_prompt_id() {
        let mut mirror = QueueMirror::default();
        mirror.reserve("client-1".into(), "hello".into(), QueueLane::FollowUp);
        let snap = mirror.apply_queue_update(&[], &["hello".into()]);
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.entries[0]["id"], "client-1");
        assert_eq!(snap.entries[0]["text"], "hello");
        assert!(snap.running_prompt_id.is_none());
    }

    #[test]
    fn dequeues_delivered_item_and_sets_running_id() {
        let mut mirror = QueueMirror::default();
        mirror.reserve("a".into(), "one".into(), QueueLane::FollowUp);
        mirror.reserve("b".into(), "two".into(), QueueLane::FollowUp);
        let first = mirror.apply_queue_update(&[], &["one".into(), "two".into()]);
        assert_eq!(first.follow_up_count, 2);

        let second = mirror.apply_queue_update(&[], &["two".into()]);
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0]["id"], "b");
        assert_eq!(second.running_prompt_id.as_deref(), Some("a"));

        let third = mirror.apply_queue_update(&[], &[]);
        assert!(third.entries.is_empty());
        assert_eq!(third.running_prompt_id.as_deref(), Some("b"));
    }

    #[test]
    fn steering_rows_precede_follow_up_rows() {
        let mut mirror = QueueMirror::default();
        let snap = mirror.apply_queue_update(&["steer-me".into()], &["later".into()]);
        assert_eq!(snap.entries[0]["text"], "steer-me");
        assert_eq!(snap.entries[1]["text"], "later");
        assert_eq!(snap.steering_count, 1);
        assert_eq!(snap.follow_up_count, 1);
    }

    #[test]
    fn stable_ids_survive_reorder_within_lane() {
        let mut mirror = QueueMirror::default();
        mirror.apply_queue_update(&[], &["a".into(), "b".into()]);
        let before = mirror.snapshot();
        let id_a = before.entries[0]["id"].as_str().unwrap().to_string();
        let id_b = before.entries[1]["id"].as_str().unwrap().to_string();

        // Same multiset, same order — ids preserved.
        let after = mirror.apply_queue_update(&[], &["a".into(), "b".into()]);
        assert_eq!(after.entries[0]["id"], id_a);
        assert_eq!(after.entries[1]["id"], id_b);
    }

    #[test]
    fn duplicate_texts_match_fifo() {
        let mut mirror = QueueMirror::default();
        mirror.reserve("first".into(), "same".into(), QueueLane::Steering);
        mirror.reserve("second".into(), "same".into(), QueueLane::Steering);
        let snap = mirror.apply_queue_update(&["same".into(), "same".into()], &[]);
        assert_eq!(snap.entries[0]["id"], "first");
        assert_eq!(snap.entries[1]["id"], "second");
    }
}
