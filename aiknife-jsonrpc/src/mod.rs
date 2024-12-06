//! A very minimal JSON-RPC implementation, with just enough functionality to implement MCP
//!
//! `jsonrpsee` is a more full-featured JSON-RPC library, but it's also more complex and as of this
//! writing does not allow for custom transports on the server side.  Given that MCP uses the stdio
//! transport, that's a deal-breaker.
//!
//! That said, this implementation does use the JSON-RPC types from `jsonrpsee-types`.
mod client;
mod server;
mod server2;
mod shared;

pub(crate) use server::*;
pub(crate) use shared::*;
