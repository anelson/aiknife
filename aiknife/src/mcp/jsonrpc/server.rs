//! JSON RPC implementation that's specific to JSON RPC servers
use std::borrow::Cow;

use super::{
    GenericResponse, Id, JsonRpcClientMessage, JsonRpcError, Notification, Request, Response,
    ResponsePayload,
};
use tracing::*;

/// A JSON-RPC service that can handle incoming connections and requests, with as little coupling
/// to the underlying transport as possible.
#[async_trait::async_trait]
pub(crate) trait JsonRpcService: Sync + Send + 'static {
    type ConnectionHandler: JsonRpcConnectionHandler;

    /// Handle a new connection to the server by creating and returning a connection handler.
    ///
    /// The lifetime of the connection handler is tied to the lifetime of the connection itself,
    /// and is guarnteed to be used only for a single connection.  However the handler still must
    /// be both `Send` and `Sync`, as there are no guarantees that each request from a connection
    /// is handled serially.
    async fn handle_connection(
        &self,
        context: ConnectionContext,
    ) -> Result<Self::ConnectionHandler, JsonRpcError>;
}

/// The handler for a single JSON-RPC connection.
///
/// Mainly this is responsible for handling incoming method calls and notifications, but the
/// implementation can also hold connection-specific state since there is one connection handler
/// per connection.
#[async_trait::async_trait]
pub(crate) trait JsonRpcConnectionHandler: Send + Sync + 'static {
    /// Handle a JSON-RPC method invoation
    async fn handle_method<'a>(
        &self,
        id: Id<'a>,
        method: Cow<'a, str>,
        params: Option<&'a serde_json::value::RawValue>,
    ) -> Result<serde_json::Value, JsonRpcError>;

    /// Handle a JSON-RPC notification (which is like a method invocation, but no response is
    /// expected).
    ///
    /// Note that this operation is fallible only so that the server implementation can properly
    /// log errors handling notifications.  No error will be returned to the client because
    /// according to the JSON RPC spec servers MUST NOT return any response to notifications
    async fn handle_notification<'a>(
        &self,
        method: Cow<'a, str>,
        params: Option<&'a serde_json::value::RawValue>,
    ) -> Result<(), JsonRpcError> {
        // By default, just implement this as a method invocation that ignores the response
        let _ = self.handle_method(Id::Null, method, params).await?;

        Ok(())
    }
}

/// Context for a single JSON-RPC connection.
///
/// TODO: fill this out.
#[derive(Clone, Debug)]
pub(crate) struct ConnectionContext;

/// JSON RPC server which implements the JSON RPC-specific plumbing, then invokes some
/// [`JsonRpcService`] trait impl to do the actual logic.
pub(crate) struct JsonRpcServer<S> {
    service: S,
}

impl<S> JsonRpcServer<S>
where
    S: JsonRpcService,
{
    pub(crate) fn new(service: S) -> Self {
        Self { service }
    }

    #[instrument(skip(self))]
    pub(crate) async fn handle_connection(
        &self,
        context: ConnectionContext,
    ) -> Result<JsonRpcServerConnection<S::ConnectionHandler>, JsonRpcError> {
        let handler = self.service.handle_connection(context).await?;

        Ok(JsonRpcServerConnection { handler })
    }
}

/// A JSON-RPC connection that is specific to JSON-RPC servers.
pub(crate) struct JsonRpcServerConnection<H> {
    handler: H,
}

impl<H> JsonRpcServerConnection<H>
where
    H: JsonRpcConnectionHandler,
{
    pub(crate) fn new(handler: H) -> Self {
        Self { handler }
    }

    /// Handle a JSON RPC request represented as a JSON string.
    ///
    /// Both success and error responses are also objects serialized to JSON
    pub(crate) async fn handle_request<'a>(&self, request: String) -> Result<String, String> {
        self.handle_request_internal(request).await
        .map_err(Self::json_rpc_error_to_string)
            .map(|response| {
                serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"error\":\"JSON serialization error while attempting to serialize response: {}\"}}", e.to_string()))
            })
    }

    /// Internal request handler that returns the Rust response and error types
    async fn handle_request_internal(
        &self,
        request: String,
    ) -> Result<Option<GenericResponse>, JsonRpcError> {
        match JsonRpcClientMessage::from_str(&request)? {
            JsonRpcClientMessage::Request(request) => {
                let id = request.id.clone().into_owned();
                // Handle the request
                let response = self.handle_method(request).await?;

                // Wrap it in the standard JSON-RPC response
                let response = Response::new(ResponsePayload::success(response), id.clone());

                Ok(Some(response))
            }
            JsonRpcClientMessage::Notification(notification) => {
                // Notifications don't get responses.  however the notification handler is
                // fallible, if it fails we want to log that fact.
                let method = notification.method.clone().into_owned();

                if let Err(e) = self.handle_notification(notification).await {
                    error!(%method, error = ?e, "Error handling notification!");
                }
                Ok(None)
            }
            JsonRpcClientMessage::InvalidRequest(invalid) => {
                // This request is mal-formed but at least is has an ID so we can reference that ID
                // in the resulting error
                let id = invalid.id.into_owned();
                let response = JsonRpcError::invalid_request(id.clone());
                Err(response)
            }
        }
    }

    #[instrument(skip_all, fields(method = %request.method, id = %request.id))]
    async fn handle_method<'a>(
        &self,
        request: Request<'a>,
    ) -> Result<serde_json::Value, JsonRpcError> {
        let Request {
            id, method, params, ..
        } = request;

        // It's not clear why jsonrpsee `Request` structs use `Option<Cow<RawValue>>` while the
        // Notification uses `Option<&RawValue>`.  For consistency, both the method and
        // notification handlers just take `Option<&RawValue>`, so jump through some hoops to hide
        // the Cow here.
        let params_ref = params.as_ref().map(|cow| cow.as_ref());

        self.handler
            .handle_method(id, method, params_ref)
            .await
            .inspect_err(|e| {
                error!(error = ?e, "Error handling method invocation");
            })
    }

    /// Handle a JSON-RPC notification (which is like a method invocation, but no response is
    /// expected).
    ///
    /// Note that this operation is fallible only so that the server implementation can properly
    /// log errors handling notifications.  No error will be returned to the client because
    /// according to the JSON RPC spec servers MUST NOT return any response to notifications
    #[instrument(skip_all, fields(method = %request.method))]
    async fn handle_notification<'a>(&self, request: Notification<'a>) -> Result<(), JsonRpcError> {
        let Notification { method, params, .. } = request;
        if let Err(e) = self.handler.handle_notification(method, params).await {
            error!(error = ?e, "Error handling notification");
        }

        Ok(())
    }

    fn json_rpc_error_to_string(error: JsonRpcError) -> String {
        let response: GenericResponse = error.into();

        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("{{\"error\":\"JSON serialization error while attempting to report an error: {}\"}}", e.to_string()))
    }
}

/// Helper for RPC server impls to concisely serialize their method responses to JSON
pub(crate) fn json_response<T: serde::Serialize>(
    id: &Id,
    response: &T,
) -> Result<serde_json::Value, JsonRpcError> {
    serde_json::to_value(response).map_err(|e| JsonRpcError::ser(e, id.clone().into_owned()))
}

/// Helper for RPC server impls to deserialize their expected parameters struct from the JSON
/// request.  Properly handles error reporting.
///
/// # Note
///
/// Calling this function assumes that the request is expected to have `params`.  If `params` are
/// missing, this will report an error.  If your request doesn't expect params, then do not call
/// this method.
pub(crate) fn expect_params<P: serde::de::DeserializeOwned>(
    id: &Id,
    params: Option<&serde_json::value::RawValue>,
) -> Result<P, JsonRpcError> {
    let params = params.ok_or_else(|| {
        error!("Expected params in request, but none were provided");
        JsonRpcError::invalid_params(id.clone().into_owned())
    })?;
    serde_json::from_str(params.get()).map_err(|e| {
        error!(error = %e, params = params.get(), "Error deserializing params");
        JsonRpcError::invalid_params(id.clone().into_owned())
    })
}
