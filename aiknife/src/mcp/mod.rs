use std::borrow::Cow;

use anyhow::Result;
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned, Id, Request, Response, ResponsePayload};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing::*;

#[allow(dead_code, irrefutable_let_patterns)]
mod types;

/// Trait representing a transport over which MCP requests and responses can be sent
#[async_trait::async_trait]
pub trait McpTransport: Send {
    /// Read the next request from the transport
    /// Returns None if the transport has been closed
    async fn read_request(&mut self) -> Result<Option<String>>;

    /// Write a response to the transport
    async fn write_response(
        &mut self,
        response: Response<'static, serde_json::Value>,
    ) -> Result<()> {
        self.write_response_string(&serde_json::to_string(&response)?)
            .await
    }

    /// Write a response to the transport
    async fn write_response_string(&mut self, response: &str) -> Result<()>;
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

    async fn write_response_string(&mut self, response: &str) -> Result<()> {
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

    async fn write_response_string(&mut self, response: &str) -> Result<()> {
        self.stream.get_mut().write_all(response.as_bytes()).await?;
        self.stream.get_mut().write_all(b"\n").await?;
        Ok(())
    }
}

/// Run the server for MCP using stdin/stdout
pub async fn serve(mut transport: Box<dyn McpTransport>) -> Result<()> {
    while let Some(request) = transport.read_request().await? {
        debug!(%request, "Received MCP request");

        match process_request(request).await {
            Ok(response) => {
                if let Some(response) = response {
                    transport.write_response_string(&response).await?;
                }
            }
            Err(e) => {
                let response: Response<serde_json::Value> = e.into();
                transport.write_response(response).await?;
            }
        }
    }

    Ok(())
}

/// Possible kinds of request from JSON-RPC clients
#[derive(Debug)]
enum JsonRpcRequest<'a> {
    /// A regular method invocation
    Request(Request<'a>),

    /// A notification, which is fire-and-forget and does not elicit a response
    ///
    /// NOTE: this weird ugly `Option<..>` crap is copied from `server.js` in the jsonrpsee-server
    /// code.  It allows to defer the parsing of the payload until later when we can determine what
    /// the expected input is.
    Notification(jsonrpsee::types::Notification<'a, Option<&'a serde_json::value::RawValue>>),

    /// An invalid request, which is a JSON-RPC error, but still has an ID field so that when we
    /// report the error we can include the ID of the request that caused it.
    InvalidRequest(jsonrpsee::types::InvalidRequest<'a>),
}

impl<'a> JsonRpcRequest<'a> {
    fn from_str(request: &'a str) -> Result<Self, JsonRpcError> {
        // Inspired by the `handle_rpc_call` function in jsonrpsee-server in `src/server.rs`

        // In short: try to parse as Request, if not then as Notification, and if not as
        // InvalidRequest
        if let Ok(request) = serde_json::from_str::<Request>(request) {
            Ok(JsonRpcRequest::Request(request))
        } else if let Ok(notification) = serde_json::from_str::<
            jsonrpsee::types::Notification<'a, Option<&'a serde_json::value::RawValue>>,
        >(request)
        {
            Ok(JsonRpcRequest::Notification(notification))
        } else {
            Ok(JsonRpcRequest::InvalidRequest(
                serde_json::from_str::<jsonrpsee::types::InvalidRequest>(request)
                    .map_err(|e| JsonRpcError::deser(e, None))?,
            ))
        }
    }
}

/// Internal error type that captures errors as JSON-RPC errors
///
/// Used only to capture error information in enough detail to generate proper JSON error
/// responses.
///
/// Uses the error codes defined in the JSON-RPC spec.
#[derive(Debug)]
struct JsonRpcError {
    code: ErrorCode,
    message: String,
    id: Option<Id<'static>>,
    data: Option<Vec<String>>,
}

impl JsonRpcError {
    fn new(
        code: ErrorCode,
        message: impl Into<String>,
        id: impl Into<Option<Id<'static>>>,
        data: impl Into<Option<Vec<String>>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            id: id.into(),
            data: data.into(),
        }
    }

    /// Make a new JSON RPC error, capturing the source error chain in the `data` field
    fn from_error(
        code: ErrorCode,
        error: impl std::error::Error,
        id: impl Into<Option<Id<'static>>>,
    ) -> Self {
        let message = error.to_string();
        let mut inner = error.source();
        let mut data = vec![];
        while let Some(e) = inner {
            data.push(e.to_string());
            inner = e.source();
        }

        Self::new(code, message, id, data)
    }

    /// Error deserializing some JSON.
    fn deser(error: serde_json::Error, id: impl Into<Option<Id<'static>>>) -> Self {
        Self::from_error(ErrorCode::ParseError, error, id)
    }

    /// Error serializing some JSON.  Less likely but still possible.
    fn ser(error: serde_json::Error, id: Id<'static>) -> Self {
        Self::from_error(ErrorCode::ParseError, error, id)
    }

    fn method_not_found(method: String, id: Id<'static>) -> Self {
        Self::new(
            ErrorCode::MethodNotFound,
            format!("Method not found: {}", method),
            id,
            None,
        )
    }

    fn invalid_params(id: Id<'static>) -> Self {
        Self::new(
            ErrorCode::InvalidParams,
            ErrorCode::InvalidParams.message(),
            id,
            None,
        )
    }

    fn invalid_request(id: Id<'static>) -> Self {
        Self::new(
            ErrorCode::InvalidRequest,
            ErrorCode::InvalidRequest.message(),
            id,
            None,
        )
    }
}

impl Into<Response<'static, serde_json::Value>> for JsonRpcError {
    fn into(self) -> Response<'static, serde_json::Value> {
        Response::new(
            ResponsePayload::error(ErrorObjectOwned::owned::<Vec<String>>(
                self.code.code(),
                self.message,
                self.data,
            )),
            self.id.unwrap_or(Id::Null),
        )
    }
}

async fn process_request(request: String) -> Result<Option<String>, JsonRpcError> {
    // Parse the request as JSON
    //
    // TODO: the spec allows for batch requests, which obviously will not be handled correctly
    // here.
    // Does MCP use them?
    //
    // if so, in the `jsonrpsee-server` code, `src/server.rs`, see the function `handle_rpc_call`

    // Try to parse this as a request, if that fails try to treat it as a notification
    match JsonRpcRequest::from_str(&request)? {
        JsonRpcRequest::Request(request) => {
            let id = request.id.clone().into_owned();
            // Handle the request
            let response = handle_request(request).await?;

            // Wrap it in the standard JSON-RPC response
            let response = Response::new(ResponsePayload::success(response), id.clone());

            // Serialize the response
            let response =
                serde_json::to_string(&response).map_err(|e| JsonRpcError::ser(e, id))?;
            Ok(Some(response))
        }
        JsonRpcRequest::Notification(_notification) => {
            // Notifications don't get responses
            error!("TODO: implement notifications!");
            Ok(None)
        }
        JsonRpcRequest::InvalidRequest(invalid) => {
            // This request is mal-formed but at least is has an ID so we can reference that ID
            // in the resulting error
            let id = invalid.id.into_owned();
            let response = JsonRpcError::invalid_request(id.clone());
            let response: Response<'_, _> = response.into();
            let response =
                serde_json::to_string(&response).map_err(|e| JsonRpcError::ser(e, id))?;
            Ok(Some(response))
        }
    }
}

/// Handle an individual MCP request
async fn handle_request(request: Request<'_>) -> Result<serde_json::Value, JsonRpcError> {
    // Handle different method calls
    let Request {
        method, params, id, ..
    } = request;

    match method.as_ref() {
        "ping" => {
            let _params = deser_params::<types::PingRequestParams>(params, &id)?;
            let response = ();
            let response = serde_json::to_value(response)
                .map_err(|e| JsonRpcError::ser(e, id.into_owned()))?;

            Ok(response)
        }
        "initialize" => {
            let params = deser_params::<types::InitializeRequestParams>(params, &id)?;
            debug!(?params, "Received initialize request");
            let response = types::InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                capabilities: types::ServerCapabilities {
                    experimental: Default::default(),
                    logging: Default::default(),
                    resources: Some(types::ServerCapabilitiesResources {
                        list_changed: Some(true),
                        subscribe: Some(true),
                    }),
                    prompts: Some(types::ServerCapabilitiesPrompts {
                        list_changed: Some(true),
                    }),
                    tools: Some(types::ServerCapabilitiesTools {
                        list_changed: Some(true),
                    }),
                },
                instructions: Some("This MCP doesn't do much, but it does it in Rust!".to_string()),
                meta: Default::default(),
                server_info: types::Implementation {
                    name: env!("CARGO_PKG_NAME").to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
            };

            let response = serde_json::to_value(&response)
                .map_err(|e| JsonRpcError::ser(e, id.into_owned()))?;
            Ok(response)
        }

        // Add more method handlers here
        _ => Err(JsonRpcError::method_not_found(
            method.to_string(),
            id.into_owned(),
        )),
    }
}

fn deser_params<P: serde::de::DeserializeOwned>(
    params: Option<Cow<'_, serde_json::value::RawValue>>,
    id: &Id<'_>,
) -> Result<P, JsonRpcError> {
    // If this is called, then the method expects params, so the lack of params is a protocol error
    let params = params.ok_or_else(|| JsonRpcError::invalid_params(id.clone().into_owned()))?;
    serde_json::from_str::<P>(params.get())
        .map_err(|e| JsonRpcError::deser(e, id.clone().into_owned()))
}

/// Helper to send error responses
fn make_error_response(error: anyhow::Error) -> Result<String> {
    let response = Response::<()>::new(
        ResponsePayload::error(ErrorObjectOwned::owned::<()>(
            ErrorCode::InternalError.code(),
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

    #[test]
    fn parse_json_rpc_notification() {
        let request = r#"    { "jsonrpc": "2.0", "method": "notifications/initialized" }"#;
        let request: JsonRpcRequest = JsonRpcRequest::from_str(request).unwrap();
        assert!(matches!(request, JsonRpcRequest::Notification(_)));
    }

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
                .write_response_string("test response")
                .await?;

            Result::<()>::Ok(())
        });

        // Client sends request
        client_transport
            .write_response_string("test request")
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
            server.write_response_string("test response").await?;

            Result::<()>::Ok(())
        });

        // Small delay to ensure server is ready
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connect client
        let mut client = UnixSocketTransport::connect(&socket_path).await?;

        // Send request
        client.write_response_string("test request").await?;

        // Read response
        let response = client.read_request().await?;
        assert_eq!(response, Some("test response\n".to_string()));

        // Wait for server to complete
        server_handle.await??;

        Ok(())
    }
}
