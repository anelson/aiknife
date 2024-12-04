use anyhow::Result;
use jsonrpsee::types as jsonrpc;
use std::borrow::Cow;
use tracing::*;

mod client;
mod server;
mod shared;
mod transport;
#[allow(dead_code, irrefutable_let_patterns)]
mod types;

use shared::{JsonRpcError, JsonRpcRequest};
pub use transport::{McpTransport, StdioTransport, StreamTransport, UnixSocketTransport};

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
                let response: jsonrpc::Response<serde_json::Value> = e.into();
                transport.write_response(response).await?;
            }
        }
    }

    Ok(())
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
            let response =
                jsonrpc::Response::new(jsonrpc::ResponsePayload::success(response), id.clone());

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
            let response: jsonrpc::Response<'_, _> = response.into();
            let response =
                serde_json::to_string(&response).map_err(|e| JsonRpcError::ser(e, id))?;
            Ok(Some(response))
        }
    }
}

/// Handle an individual MCP request
async fn handle_request(request: jsonrpc::Request<'_>) -> Result<serde_json::Value, JsonRpcError> {
    // Handle different method calls
    let jsonrpc::Request {
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
    id: &jsonrpc::Id<'_>,
) -> Result<P, JsonRpcError> {
    // If this is called, then the method expects params, so the lack of params is a protocol error
    let params = params.ok_or_else(|| JsonRpcError::invalid_params(id.clone().into_owned()))?;
    serde_json::from_str::<P>(params.get())
        .map_err(|e| JsonRpcError::deser(e, id.clone().into_owned()))
}

/// Helper to send error responses
fn make_error_response(error: anyhow::Error) -> Result<String> {
    let response = jsonrpc::Response::<()>::new(
        jsonrpc::ResponsePayload::error(jsonrpc::ErrorObjectOwned::owned::<()>(
            jsonrpc::ErrorCode::InternalError.code(),
            format!("{:?}", error),
            None,
        )),
        jsonrpc::Id::Null,
    );

    Ok(serde_json::to_string(&response)?)
}
