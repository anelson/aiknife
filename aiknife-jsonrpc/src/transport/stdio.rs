//! Implementation of the so-called "stdio" transport, which is another way of saying a transport
//! in which the entire process is handling a single client connection, with client requests coming
//! in on stdin, responses going out on stdout, and log events (if any) usually logged to stderr.
//!
//! This is described in the MCP spec at <https://spec.modelcontextprotocol.io/specification/basic/transports/#stdio>

pub struct StdioTransport {}
