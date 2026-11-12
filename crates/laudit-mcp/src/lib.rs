//! MCP stdio server exposing laudit engine tools to AI agents.
//!
//! The transport is deliberately separate from the engine: this crate speaks
//! MCP over newline-delimited JSON-RPC 2.0 on stdio and delegates all work to
//! `laudit-core` handlers. Every value returned to the client is redacted
//! before it leaves the process (D11).

pub mod protocol;
pub mod server;
pub mod tools;

pub use server::{run, serve, Server};
