//! Native agent engine. No terminal, process-global configuration, or implicit network access.
pub mod auth;
mod codemode;
pub mod mcp;
pub mod mcp_oauth;
pub mod ollama;
mod provider;
mod registry;
mod retry;
pub mod secure;
mod session;
mod skills;
mod sse;
mod tools;

pub use provider::Provider;
pub use registry::{ImageAttachment, Tool, ToolContext, ToolDefinition, ToolFuture, ToolOutput};
pub use session::{Event, Message, Preferences, Session, SessionConfig, ToolCall};
pub use tokio_util::sync::CancellationToken;
