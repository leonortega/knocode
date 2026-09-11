use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use knocode_core::CorrelationId;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

// ── Events ──────────────────────────────────────────────────────────────

/// All observable runtime events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum RuntimeEvent {
    ContextBuilt {
        correlation_id: CorrelationId,
        token_counts: TokenCounts,
        file_count: usize,
        latency_ms: u64,
    },
    SkillActivated {
        correlation_id: CorrelationId,
        skill_name: String,
        match_score: f64,
    },
    RepositoryUpdated {
        files_indexed: usize,
        symbols_extracted: usize,
        duration_ms: u64,
    },
    ToolExecuted {
        tool_name: String,
        original_tokens: usize,
        compressed_tokens: usize,
        ratio: f64,
    },
    ModelSelected {
        correlation_id: CorrelationId,
        model: String,
        tier: String,
        score: f64,
        reasoning: String,
    },
    ResponseGenerated {
        correlation_id: CorrelationId,
        hook_type: String,
        latency_ms: u64,
        error: Option<String>,
    },
    MemorySaved {
        entry_id: String,
        namespace: String,
        key: String,
    },
}

/// Token count breakdown for an event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCounts {
    pub total: usize,
    pub by_source: std::collections::HashMap<String, usize>,
}

/// Timestamped event for the in-memory buffer
#[derive(Debug, Clone)]
pub struct TimestampedEvent {
    pub event: RuntimeEvent,
    pub timestamp: DateTime<Utc>,
}

// ── Event Bus ───────────────────────────────────────────────────────────

const DEFAULT_BUFFER_SIZE: usize = 1000;

/// In-memory event bus for runtime observability
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<RuntimeEvent>,
    buffer: Arc<Mutex<VecDeque<TimestampedEvent>>>,
    buffer_size: usize,
}

impl EventBus {
    /// Create a new Event Bus with default buffer size (1000 events)
    pub fn new() -> Self {
        Self::with_buffer_size(DEFAULT_BUFFER_SIZE)
    }

    /// Create a new Event Bus with a custom buffer size
    pub fn with_buffer_size(buffer_size: usize) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(buffer_size))),
            buffer_size,
        }
    }

    /// Emit an event (fire-and-forget). Returns Ok(()) even if no subscribers.
    pub fn emit(&self, event: RuntimeEvent) {
        // Store in buffer
        {
            let mut buffer = self.buffer.lock().unwrap();
            let timestamped = TimestampedEvent {
                event: event.clone(),
                timestamp: Utc::now(),
            };
            if buffer.len() >= self.buffer_size {
                buffer.pop_front();
            }
            buffer.push_back(timestamped);
        }

        // Broadcast to subscribers (ignore error if no receivers)
        let _ = self.sender.send(event);
    }

    /// Subscribe to events, returning a broadcast receiver
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }

    /// Get the last N events from the buffer
    pub fn get_recent_events(&self, n: usize) -> Vec<TimestampedEvent> {
        let buffer = self.buffer.lock().unwrap();
        let len = buffer.len();
        buffer
            .iter()
            .skip(len.saturating_sub(n))
            .cloned()
            .collect()
    }

    /// Get events matching a specific correlation ID
    pub fn get_events_by_correlation(&self, id: &CorrelationId) -> Vec<TimestampedEvent> {
        let buffer = self.buffer.lock().unwrap();
        buffer
            .iter()
            .filter(|te| match &te.event {
                RuntimeEvent::ContextBuilt { correlation_id, .. } => correlation_id == id,
                RuntimeEvent::SkillActivated { correlation_id, .. } => correlation_id == id,
                RuntimeEvent::ModelSelected { correlation_id, .. } => correlation_id == id,
                RuntimeEvent::ResponseGenerated { correlation_id, .. } => correlation_id == id,
                _ => false,
            })
            .cloned()
            .collect()
    }

    /// Get the total number of events in the buffer
    pub fn buffer_len(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_and_buffer() {
        let bus = EventBus::new();
        let event = RuntimeEvent::ContextBuilt {
            correlation_id: CorrelationId::new(),
            token_counts: TokenCounts {
                total: 8000,
                by_source: std::collections::HashMap::new(),
            },
            file_count: 5,
            latency_ms: 12,
        };
        bus.emit(event);
        assert_eq!(bus.buffer_len(), 1);
    }

    #[tokio::test]
    async fn test_subscribe_receives_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let event = RuntimeEvent::ToolExecuted {
            tool_name: "read_file".to_string(),
            original_tokens: 1000,
            compressed_tokens: 500,
            ratio: 0.5,
        };
        bus.emit(event);

        let received = rx.recv().await.unwrap();
        match received {
            RuntimeEvent::ToolExecuted {
                tool_name,
                original_tokens,
                compressed_tokens,
                ratio,
            } => {
                assert_eq!(tool_name, "read_file");
                assert_eq!(original_tokens, 1000);
                assert_eq!(compressed_tokens, 500);
                assert!((ratio - 0.5).abs() < f64::EPSILON);
            }
            _ => panic!("Expected ToolExecuted"),
        }
    }

    #[test]
    fn test_buffer_overflow() {
        let bus = EventBus::with_buffer_size(3);
        for i in 0..5 {
            bus.emit(RuntimeEvent::MemorySaved {
                entry_id: format!("entry_{}", i),
                namespace: "test".to_string(),
                key: format!("key_{}", i),
            });
        }
        assert_eq!(bus.buffer_len(), 3);
        let events = bus.get_recent_events(10);
        assert_eq!(events.len(), 3);
        // Should contain the last 3 events (2, 3, 4)
        match &events[0].event {
            RuntimeEvent::MemorySaved { entry_id, .. } => assert_eq!(entry_id, "entry_2"),
            _ => panic!("Expected MemorySaved"),
        }
    }

    #[test]
    fn test_get_recent_events() {
        let bus = EventBus::new();
        for _ in 0..5 {
            bus.emit(RuntimeEvent::MemorySaved {
                entry_id: "id".to_string(),
                namespace: "ns".to_string(),
                key: "k".to_string(),
            });
        }
        let recent = bus.get_recent_events(3);
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_get_events_by_correlation() {
        let bus = EventBus::new();
        let id = CorrelationId::new();

        bus.emit(RuntimeEvent::ContextBuilt {
            correlation_id: id.clone(),
            token_counts: TokenCounts {
                total: 0,
                by_source: std::collections::HashMap::new(),
            },
            file_count: 0,
            latency_ms: 0,
        });

        // Different correlation ID
        bus.emit(RuntimeEvent::ContextBuilt {
            correlation_id: CorrelationId::new(),
            token_counts: TokenCounts {
                total: 0,
                by_source: std::collections::HashMap::new(),
            },
            file_count: 0,
            latency_ms: 0,
        });

        let matching = bus.get_events_by_correlation(&id);
        assert_eq!(matching.len(), 1);
    }

    #[test]
    fn test_event_serialization() {
        let event = RuntimeEvent::ModelSelected {
            correlation_id: CorrelationId::new(),
            model: "gpt-4o".to_string(),
            tier: "balanced".to_string(),
            score: 0.5,
            reasoning: "moderate complexity".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: RuntimeEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            RuntimeEvent::ModelSelected { model, tier, .. } => {
                assert_eq!(model, "gpt-4o");
                assert_eq!(tier, "balanced");
            }
            _ => panic!("Expected ModelSelected"),
        }
    }
}
