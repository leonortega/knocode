// knocode-daemon library surface — enables integration tests (TASK-035) to boot the
// HTTP router directly while keeping the `knocode-daemon` binary thin.
pub mod http_server;
pub mod lifecycle;
pub mod mcp;
pub mod metrics;
pub mod ratelimit;
