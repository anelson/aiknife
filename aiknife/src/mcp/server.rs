//! Tiny framework for making Model Context Protocol servers in Rust.
//!
//! Uses the JSON RPC impl in [`crate::mcp::jsonrpc`] to implement the underlying JSON RPC
//! protocol.

use super::jsonrpc;

#[derive(Debug)]
enum State {
    Uninitialized,
    Initialize,
}

#[derive(Debug)]
pub(crate) struct McpServer {
    state: State,
}
