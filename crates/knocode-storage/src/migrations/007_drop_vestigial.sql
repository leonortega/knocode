-- Migration 007: drop vestigial model-map columns.
-- The LLM Model Router / LiteLLM was removed (v0.8.6, LLM_ROUTING_REMOVAL.md); nothing
-- reads a model or tier anymore. token_usage.model/tier survived as NOT NULL leftovers —
-- removed here for both fresh installs (001 creates them, 007 drops) and existing DBs.
ALTER TABLE token_usage DROP COLUMN model;
ALTER TABLE token_usage DROP COLUMN tier;
