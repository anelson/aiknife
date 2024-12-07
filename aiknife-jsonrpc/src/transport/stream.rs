//! Defines a JSON RPC transport that communicates with a client over two async streams, one for reading
//! and one for writing.
//!
//! This is used to implement the stdio transport, the Unix Domain Socket transport, and also is
//! useful for creating tests that simulate a client and server communicating over a network.

use crate::server::{JsonRpcServer, JsonRpcServerConnection, JsonRpcService};
use anyhow::Result;
use futures::StreamExt;
use std::fmt::{Debug, Formatter};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};
use tracing::*;

pub struct StreamTransport<R, W, S> {
    reader: R,
    writer: W,
    server: JsonRpcServer<S>,
}

impl<R, W, S> Debug for StreamTransport<R, W, S>
where
    R: Debug,
    W: Debug,
    JsonRpcServer<S>: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamTransport")
            .field("reader", &self.reader)
            .field("writer", &self.writer)
            .field("server", &self.server)
            .finish()
    }
}

impl<R, W, S> StreamTransport<R, W, S>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    S: JsonRpcService,
{
    pub fn new(reader: R, writer: W, server: JsonRpcServer<S>) -> Self {
        Self {
            reader,
            writer,
            server,
        }
    }

    /// Start serving the JSON RPC service over this transport.
    ///
    /// Stops when the cancellation token is triggered, or the reader stream ends.
    pub async fn serve(
        &mut self,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        // For stream transports, there is only ever one connection, which services the entire
        // stream
        let (conn, mut receiver) = self.server.handle_connection().await?;
        let mut pending_requests = futures::stream::FuturesUnordered::new();

        loop {
            let mut line = String::new();
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Cancellation requested; aborting");
                    return Ok(());
                },
                result = self.reader.read_line(&mut line) => {
                    match result? {
                        0 => {
                            // EOF
                            debug!("EOF on reader; stopping stream");
                            return Ok(());
                        }
                        _ => {
                            // Got a request on a line of input
                            // create a future to process that line and add it to the collection
                            // of pending requests.  In this way we can process multiple requests
                            // concurrently.
                            pending_requests.push(conn.handle_request(line));
                        }
                    }
                },
                result = pending_requests.next(), if !pending_requests.is_empty() => {
                    // A future handling a previously-received request has completed.
                    let result = result.expect("BUG: next() is only called if there is at least one future to poll!");

                    match result {
                        Ok(Some(response)) | Err(response) => {
                            // Method invocation succeeded or failed; either way send the response back
                            self.writer.write_all(response.as_bytes()).await?;
                            self.writer.write_all(b"\n").await?;
                            self.writer.flush().await?;
                        },
                        Ok(None) => {
                            // The request was a notification, per the spec no response
                            // may be sent to notifications
                        }
                    }
                }
                result = receiver.recv() => {
                    match result {
                        Some(notification) => {
                            // Server-initiated notification, send to the client
                            self.writer.write_all(notification.as_bytes()).await?;
                            self.writer.write_all(b"\n").await?;
                            self.writer.flush().await?;
                        }
                        None => {
                            debug!("Server connection closed; stopping stream");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}
