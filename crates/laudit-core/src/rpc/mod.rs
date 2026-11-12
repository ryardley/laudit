//! JSON-RPC 2.0 over newline-delimited stdio.

mod protocol;
mod server;

pub use protocol::{error_response, success_response, ErrorCode, RpcError, JSONRPC_VERSION};
pub use server::{run, serve, Client, Handler, HandlerFuture, Server, ServerContext};
