//! Native agent engine. No terminal, process-global configuration, or implicit network access.
pub mod auth;
mod provider;
mod session;
mod sse;
mod tools;

pub use provider::Provider;
pub use session::{Event, Message, Preferences, Session, SessionConfig, ToolCall};
pub use tokio_util::sync::CancellationToken;
