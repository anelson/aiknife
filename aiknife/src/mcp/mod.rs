use std::borrow::Cow;

use anyhow::Result;
use jsonrpsee::types::{ErrorObjectOwned, Id, Request, Response, ResponsePayload};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing::*;

#[allow(dead_code, irrefutable_let_patterns)]
mod types;

/// Trait representing a transport over which MCP requests and responses can be sent
#[async_trait::async_trait]
pub trait McpTransport {
    /// Read the next request from the transport
    /// Returns None if the transport has been closed
    async fn read_request(&mut self) -> Result<Option<String>>;

    /// Write a response to the transport
    async fn write_response(&mut self, response: String) -> Result<()>;
}

/// Implementation of McpTransport for any AsyncRead/AsyncWrite pair
pub struct StreamTransport<R, W>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    reader: R,
    writer: W,
}

impl<R, W> StreamTransport<R, W>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

#[async_trait::async_trait]
impl<R, W> McpTransport for StreamTransport<R, W>
where
    R: AsyncBufReadExt + Unpin + Send,
    W: AsyncWriteExt + Unpin + Send,
{
    async fn read_request(&mut self) -> Result<Option<String>> {
        let mut line = String::new();
        let bytes_read = self.reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Ok(None);
        }
        Ok(Some(line))
    }

    async fn write_response(&mut self, response: String) -> Result<()> {
        self.writer.write_all(response.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

// Create a type alias for the common stdio case
pub type StdioTransport = StreamTransport<
    tokio::io::BufReader<tokio::io::Stdin>,
    tokio::io::BufWriter<tokio::io::Stdout>,
>;

// Update the constructor for StdioTransport
impl StdioTransport {
    pub fn stdio(stdin: tokio::io::Stdin, stdout: tokio::io::Stdout) -> Self {
        Self::new(
            tokio::io::BufReader::new(stdin),
            tokio::io::BufWriter::new(stdout),
        )
    }
}

/// Implementation of McpTransport for Unix domain sockets
pub struct UnixSocketTransport {
    stream: tokio::io::BufReader<tokio::net::UnixStream>,
}

impl UnixSocketTransport {
    pub async fn bind<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        // Remove existing socket file if it exists
        if path.as_ref().exists() {
            tokio::fs::remove_file(&path).await?;
        }

        let listener = tokio::net::UnixListener::bind(&path)?;
        let stream = listener.accept().await?.0;

        Ok(Self {
            stream: tokio::io::BufReader::new(stream),
        })
    }

    pub async fn connect<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(Self {
            stream: tokio::io::BufReader::new(stream),
        })
    }
}

#[async_trait::async_trait]
impl McpTransport for UnixSocketTransport {
    async fn read_request(&mut self) -> Result<Option<String>> {
        let mut line = String::new();
        let bytes_read = self.stream.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Ok(None);
        }
        Ok(Some(line))
    }

    async fn write_response(&mut self, response: String) -> Result<()> {
        self.stream.get_mut().write_all(response.as_bytes()).await?;
        self.stream.get_mut().write_all(b"\n").await?;
        Ok(())
    }
}

/// Run the server for MCP using stdin/stdout
pub async fn serve(mut transport: Box<dyn McpTransport>) -> Result<()> {
    while let Some(request) = transport.read_request().await? {
        debug!(%request, "Received MCP request");

        let response = process_request(request)
            .await
            .or_else(make_error_response)?;

        transport.write_response(response).await?;
    }

    Ok(())
}

async fn process_request(request: String) -> Result<String> {
    // Parse the request as JSON
    let request: Request = serde_json::from_str(&request)?;

    // Handle the request
    let response = handle_request(request).await?;

    // Serialize the response
    let response = serde_json::to_string(&response)?;

    Ok(response)
}

/// Handle an individual MCP request
async fn handle_request(request: Request<'_>) -> Result<Response<'static, serde_json::Value>> {
    // Handle different method calls

    // TODO: jsonrpsee isn't going to work.  Its internals are not exposed enough to implement a
    // custom transport on the server side.  So I'll just roll my own for now.
    let method = request.method.as_ref();
    match method {
        "ping" => Ok(Response::<serde_json::Value>::new(
            ResponsePayload::Success(Cow::Owned(json! { "pong" })),
            request.id,
        )
        .into_owned()),

        // Add more method handlers here
        _ => {
            anyhow::bail!("Unknown method: {}", method);
        }
    }
}

/// Helper to send error responses
fn make_error_response(error: anyhow::Error) -> Result<String> {
    let response = Response::<()>::new(
        ResponsePayload::error(ErrorObjectOwned::owned::<()>(
            -32000,
            format!("{:?}", error),
            None,
        )),
        Id::Null,
    );

    Ok(serde_json::to_string(&response)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

    #[tokio::test]
    async fn test_stream_transport() -> Result<()> {
        let (client_rx, server_tx) = tokio::io::duplex(64);
        let (server_rx, client_tx) = tokio::io::duplex(64);

        // Server transport
        let mut server_transport =
            StreamTransport::new(BufReader::new(server_rx), BufWriter::new(server_tx));

        // Client transport
        let mut client_transport =
            StreamTransport::new(BufReader::new(client_rx), BufWriter::new(client_tx));

        // Spawn server task
        let server_handle = tokio::spawn(async move {
            // Read the request
            let request = server_transport.read_request().await?;
            assert_eq!(request, Some("test request\n".to_string()));

            // Send response
            server_transport
                .write_response("test response".to_string())
                .await?;

            Result::<()>::Ok(())
        });

        // Client sends request
        client_transport
            .write_response("test request".to_string())
            .await?;

        // Client reads response
        let response = client_transport.read_request().await?;
        assert_eq!(response, Some("test response\n".to_string()));

        // Wait for server to complete
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn test_unix_socket_transport() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_path_buf();

        // Start server in background task
        let server_path = socket_path.clone();
        let server_handle = tokio::spawn(async move {
            let mut server = UnixSocketTransport::bind(&server_path).await?;

            // Read request
            let request = server.read_request().await?;
            assert_eq!(request, Some("test request\n".to_string()));

            // Send response
            server.write_response("test response").await?;

            Result::<()>::Ok(())
        });

        // Small delay to ensure server is ready
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connect client
        let mut client = UnixSocketTransport::connect(&socket_path).await?;

        // Send request
        client.write_response("test request").await?;

        // Read response
        let response = client.read_request().await?;
        assert_eq!(response, Some("test response\n".to_string()));

        // Wait for server to complete
        server_handle.await??;

        Ok(())
    }
}
