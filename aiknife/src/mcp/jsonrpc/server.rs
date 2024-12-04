//! JSON RPC implementation that's specific to JSON RPC servers
use super::shared as jsonrpc;
use std::borrow::Cow;
use tracing::*;

type NotificationSender = tokio::sync::mpsc::Sender<String>;

/// Channel receiver for notifications sent asynchronously from the server, to the client, but not
/// in response to a specific client request.
pub(crate) type NotificationReceiver = tokio::sync::mpsc::Receiver<String>;

/// A JSON-RPC service that can handle incoming connections and requests, with as little coupling
/// to the underlying transport as possible.
#[async_trait::async_trait]
pub(crate) trait JsonRpcService: Sync + Send + 'static {
    type ConnectionHandler: JsonRpcConnectionHandler;

    /// The maximum number of pending notifications that can be buffered for a single connection.
    ///
    /// This translates into the bound for the channel that is used to send notifications back to
    /// the client.
    ///
    /// Most implementations should not need to modify this.
    fn max_pending_notifications(&self) -> usize {
        100
    }

    /// Handle a new connection to the server by creating and returning a connection handler.
    ///
    /// The lifetime of the connection handler is tied to the lifetime of the connection itself,
    /// and is guarnteed to be used only for a single connection.  However the handler still must
    /// be both `Send` and `Sync`, as there are no guarantees that each request from a connection
    /// is handled serially.
    async fn handle_connection(
        &self,
        context: ConnectionContext,
    ) -> Result<Self::ConnectionHandler, jsonrpc::JsonRpcError>;
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
        id: jsonrpc::Id<'a>,
        method: Cow<'a, str>,
        params: Option<&'a serde_json::value::RawValue>,
    ) -> Result<serde_json::Value, jsonrpc::JsonRpcError>;

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
    ) -> Result<(), jsonrpc::JsonRpcError> {
        // By default, just implement this as a method invocation that ignores the response
        let _ = self
            .handle_method(jsonrpc::Id::Null, method, params)
            .await?;

        Ok(())
    }
}

/// Context for a single JSON-RPC connection.
///
/// TODO: fill this out.
#[derive(Clone, Debug)]
pub(crate) struct ConnectionContext {
    server_notification_sender: NotificationSender,
}

impl ConnectionContext {
    /// Send a notification from the server back to the client.
    ///
    /// Because notifications are one-way, this method is infallible.  There are some ways that the
    /// notification can fail to send, the most likely of which is that the client has
    /// disconnected, or will disconnect shortly after the notification is written into its buffer.
    ///
    /// Errors that are encountered are logged, but not passed back to the caller
    pub(crate) async fn send_notification<'a, T: serde::Serialize + 'a>(
        &self,
        method: &'a str,
        params: impl Into<Option<&'a T>>,
    ) {
        #[derive(serde::Serialize)]
        struct JsonRpcNotification<'a> {
            jsonrpc: jsonrpc::TwoPointZero,
            method: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            params: Option<serde_json::Value>,
        }

        fn serialize_notification<'a, T: serde::Serialize + 'a>(
            method: &'a str,
            params: impl Into<Option<&'a T>>,
        ) -> Result<String, jsonrpc::JsonRpcError> {
            let params = params.into();
            let params = if let Some(params) = params {
                Some(
                    serde_json::to_value(params)
                        .map_err(|e| jsonrpc::JsonRpcError::ser(e, jsonrpc::Id::Null))?,
                )
            } else {
                None
            };

            let notification = JsonRpcNotification {
                jsonrpc: jsonrpc::TwoPointZero,
                method,
                params,
            };
            serde_json::to_string(&notification)
                .map_err(|e| jsonrpc::JsonRpcError::ser(e, jsonrpc::Id::Null))
        }

        match serialize_notification(method, params) {
            Ok(notification) => {
                if (self.server_notification_sender.send(notification).await).is_err() {
                    error!(
                        method,
                        params = std::any::type_name::<T>(),
                        "Notification not sent; receiver no longer listening"
                    );
                }
            }
            Err(e) => {
                error!(error = ?e, method, params = std::any::type_name::<T>(), "Error serializing notification");
            }
        }
    }
}

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

    /// Start a new connection handler for what is presumed to be a new connection, however the
    /// transport defines that concept
    ///
    /// Returns a tuple:
    ///
    /// - an instance of [`JsonRpcServerConnection`] that can be used to handle requests received
    ///   from the client on this connection.
    /// - an instance of [`NotifactionReceiver`] that can be used to receive notifications from the
    ///   server that is handling this connection.  Notifications can be created at any time and
    ///   are not necessarily in response to a specific client request, so the transport needs some
    ///   mechanism to receive them from this receiver and forward them to the client.
    ///
    ///   The transport-specific host of this server should launch an async task that continuously
    ///   polls this receiver and forwards the notifications to the corresponding client, for the
    ///   duration of the connection.
    #[instrument(skip(self))]
    pub(crate) async fn handle_connection(
        &self,
    ) -> Result<
        (
            JsonRpcServerConnection<S::ConnectionHandler>,
            NotificationReceiver,
        ),
        jsonrpc::JsonRpcError,
    > {
        let (sender, receiver) =
            tokio::sync::mpsc::channel(self.service.max_pending_notifications());
        let context = ConnectionContext {
            server_notification_sender: sender.clone(),
        };
        let handler = self.service.handle_connection(context).await?;

        Ok((JsonRpcServerConnection::new(handler, sender), receiver))
    }
}

/// A JSON-RPC connection that is specific to JSON-RPC servers.
pub(crate) struct JsonRpcServerConnection<H> {
    handler: H,
    server_notification_sender: NotificationSender,
}

impl<H> JsonRpcServerConnection<H>
where
    H: JsonRpcConnectionHandler,
{
    fn new(handler: H, server_notification_sender: NotificationSender) -> Self {
        Self {
            handler,
            server_notification_sender,
        }
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

    /// Internal request handler that returns the Rust response and error types, which makes the
    /// code more ergonomic.  Serializing success and failure is handled separately by the caller
    async fn handle_request_internal(
        &self,
        request: String,
    ) -> Result<Option<jsonrpc::GenericResponse>, jsonrpc::JsonRpcError> {
        match jsonrpc::JsonRpcClientMessage::from_str(&request)? {
            jsonrpc::JsonRpcClientMessage::Request(request) => {
                let id = request.id.clone().into_owned();
                // Handle the request
                let response = self.handle_method(request).await?;

                // Wrap it in the standard JSON-RPC response
                let response =
                    jsonrpc::Response::new(jsonrpc::ResponsePayload::success(response), id.clone());

                Ok(Some(response))
            }
            jsonrpc::JsonRpcClientMessage::Notification(notification) => {
                // Notifications don't get responses.  however the notification handler is
                // fallible, if it fails we want to log that fact.
                let method = notification.method.clone().into_owned();

                if let Err(e) = self.handle_notification(notification).await {
                    error!(%method, error = ?e, "Error handling notification!");
                }
                Ok(None)
            }
            jsonrpc::JsonRpcClientMessage::InvalidRequest(invalid) => {
                // This request is mal-formed but at least is has an ID so we can reference that ID
                // in the resulting error
                let id = invalid.id.into_owned();
                let response = jsonrpc::JsonRpcError::invalid_request(id.clone());
                Err(response)
            }
        }
    }

    #[instrument(skip_all, fields(method = %request.method, id = %request.id))]
    async fn handle_method<'a>(
        &self,
        request: jsonrpc::Request<'a>,
    ) -> Result<serde_json::Value, jsonrpc::JsonRpcError> {
        let jsonrpc::Request {
            id, method, params, ..
        } = request;

        // It's not clear why jsonrpsee `jsonrpc::Request` structs use `Option<Cow<RawValue>>` while the
        // jsonrpc::Notification uses `Option<&RawValue>`.  For consistency, both the method and
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
    async fn handle_notification<'a>(
        &self,
        request: jsonrpc::Notification<'a>,
    ) -> Result<(), jsonrpc::JsonRpcError> {
        let jsonrpc::Notification { method, params, .. } = request;
        if let Err(e) = self.handler.handle_notification(method, params).await {
            error!(error = ?e, "Error handling notification");
        }

        Ok(())
    }

    fn json_rpc_error_to_string(error: jsonrpc::JsonRpcError) -> String {
        let response: jsonrpc::GenericResponse = error.into();

        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("{{\"error\":\"JSON serialization error while attempting to report an error: {}\"}}", e.to_string()))
    }
}

/// Helper for RPC server impls to concisely serialize their method responses to JSON
pub(crate) fn json_response<T: serde::Serialize>(
    id: &jsonrpc::Id,
    response: &T,
) -> Result<serde_json::Value, jsonrpc::JsonRpcError> {
    serde_json::to_value(response)
        .map_err(|e| jsonrpc::JsonRpcError::ser(e, id.clone().into_owned()))
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
    id: &jsonrpc::Id,
    params: Option<&serde_json::value::RawValue>,
) -> Result<P, jsonrpc::JsonRpcError> {
    let params = params.ok_or_else(|| {
        error!("Expected params in request, but none were provided");
        jsonrpc::JsonRpcError::invalid_params(id.clone().into_owned())
    })?;
    serde_json::from_str(params.get()).map_err(|e| {
        error!(error = %e, params = params.get(), "Error deserializing params");
        jsonrpc::JsonRpcError::invalid_params(id.clone().into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use expectorate::assert_contents;
    use serde_json::json;

    // Mock connection handler for testing
    struct MockHandler {
        context: ConnectionContext,
    }

    impl MockHandler {
        fn new(context: ConnectionContext) -> Self {
            Self { context }
        }
    }

    #[async_trait::async_trait]
    impl JsonRpcConnectionHandler for MockHandler {
        async fn handle_method<'a>(
            &self,
            id: jsonrpc::Id<'a>,
            method: Cow<'a, str>,
            params: Option<&'a serde_json::value::RawValue>,
        ) -> Result<serde_json::Value, jsonrpc::JsonRpcError> {
            match method.as_ref() {
                "echo" => {
                    let params = expect_params::<serde_json::Value>(&id, params)?;
                    Ok(params)
                }
                "add" => {
                    #[derive(serde::Deserialize)]
                    struct AddParams {
                        a: i32,
                        b: i32,
                    }
                    let params = expect_params::<AddParams>(&id, params)?;
                    Ok(json!(params.a + params.b))
                }
                "trigger_server_notification" => {
                    // Send a notification from server to client
                    self.context
                        .send_notification(
                            "server_event",
                            &json!({"message": "Hello from server!"}),
                        )
                        .await;
                    Ok(json!("notification_sent"))
                }
                "trigger_multiple_notifications" => {
                    // Send multiple notifications to test ordering
                    self.context
                        .send_notification("server_event", &json!({"count": 1}))
                        .await;
                    self.context
                        .send_notification("server_event", &json!({"count": 2}))
                        .await;
                    self.context
                        .send_notification("server_event", &json!({"count": 3}))
                        .await;
                    Ok(json!("notifications_sent"))
                }
                "trigger_parameterless_notification" => {
                    // Send a notification without any parameters
                    self.context
                        .send_notification::<()>("server_event", None)
                        .await;
                    Ok(json!("parameterless_notification_sent"))
                }
                "error" => Err(jsonrpc::JsonRpcError::internal_anyhow_error(
                    id.into_owned(),
                    anyhow::anyhow!("Test error!"),
                )),
                unknown => Err(jsonrpc::JsonRpcError::method_not_found(
                    unknown.to_string(),
                    id.into_owned(),
                )),
            }
        }

        async fn handle_notification<'a>(
            &self,
            method: Cow<'a, str>,
            _params: Option<&'a serde_json::value::RawValue>,
        ) -> Result<(), jsonrpc::JsonRpcError> {
            match method.as_ref() {
                "notify" => Ok(()),
                unknown => Err(jsonrpc::JsonRpcError::method_not_found(
                    unknown.to_string(),
                    jsonrpc::Id::Null,
                )),
            }
        }
    }

    // Helper function to create a server connection for testing
    fn create_test_connection() -> (JsonRpcServerConnection<MockHandler>, NotificationReceiver) {
        let (sender, receiver) = tokio::sync::mpsc::channel(100);
        let context = ConnectionContext {
            server_notification_sender: sender.clone(),
        };
        (
            JsonRpcServerConnection::new(MockHandler::new(context), sender),
            receiver,
        )
    }

    async fn assert_response(request: &str, test_name: &str) {
        let (conn, _receiver) = create_test_connection();
        let response = match conn.handle_request(request.to_string()).await {
            Ok(response) => response,
            Err(err) => err,
        };

        // Parse and re-serialize to normalize field order and whitespace
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        let normalized = serde_json::to_string_pretty(&parsed).unwrap();

        assert_contents(
            format!("src/mcp/jsonrpc/testdata/{}.json", test_name),
            &normalized,
        );
    }

    #[tokio::test]
    async fn test_valid_request() {
        // Test echo method
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "params": "hello", "id": 1}"#,
            "echo_response",
        )
        .await;

        // Test add method
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "add", "params": {"a": 2, "b": 3}, "id": 2}"#,
            "add_response",
        )
        .await;
    }

    #[tokio::test]
    async fn test_notification() {
        // Test valid notification
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "notify"}"#,
            "notify_response",
        )
        .await;

        // Test invalid notification method
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "invalid_notify"}"#,
            "invalid_notify_response",
        )
        .await;
    }

    #[tokio::test]
    async fn test_error_cases() {
        // Test method not found
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "definitetly_non_existent", "id": 1}"#,
            "method_not_found_response",
        )
        .await;

        // Test invalid params
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "add", "params": "invalid", "id": 2}"#,
            "invalid_params_response",
        )
        .await;

        // Test reporting of an internal server error
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "error", "id": 3}"#,
            "internal_error_response",
        )
        .await;
    }

    #[tokio::test]
    async fn test_invalid_requests() {
        // Test invalid JSON
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "id": 1"#,
            "invalid_json_response",
        )
        .await;

        // Test missing jsonrpc version
        assert_response(r#"{"method": "echo", "id": 1}"#, "missing_version_response").await;

        // Test invalid version
        assert_response(
            r#"{"jsonrpc": "1.0", "method": "echo", "id": 1}"#,
            "invalid_version_response",
        )
        .await;
    }

    #[tokio::test]
    async fn test_id_types() {
        // Test numeric id
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "params": "test", "id": 1}"#,
            "numeric_id_response",
        )
        .await;

        // Test string id
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "params": "test", "id": "abc"}"#,
            "string_id_response",
        )
        .await;

        // Test null id
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "params": "test", "id": null}"#,
            "null_id_response",
        )
        .await;
    }

    #[tokio::test]
    async fn test_params_variations() {
        // Test with array params
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "params": [1,2,3], "id": 1}"#,
            "array_params_response",
        )
        .await;

        // Test with object params
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "params": {"key": "value"}, "id": 2}"#,
            "object_params_response",
        )
        .await;

        // Test with no params
        assert_response(
            r#"{"jsonrpc": "2.0", "method": "echo", "id": 3}"#,
            "no_params_response",
        )
        .await;
    }

    #[tokio::test]
    async fn test_server_notifications() {
        let (conn, mut receiver) = create_test_connection();

        // Test single notification
        let response = conn
            .handle_request(
                r#"{"jsonrpc": "2.0", "method": "trigger_server_notification", "id": 1}"#
                    .to_string(),
            )
            .await
            .unwrap();
        let notification = receiver.recv().await.expect("Should receive notification");

        // Parse and normalize the notification JSON
        let parsed: serde_json::Value = serde_json::from_str(&notification).unwrap();
        let normalized = serde_json::to_string_pretty(&parsed).unwrap();
        assert_contents(
            "src/mcp/jsonrpc/testdata/single_server_notification.json",
            &normalized,
        );

        // Parse and normalize the response JSON
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        let normalized = serde_json::to_string_pretty(&parsed).unwrap();
        assert_contents(
            "src/mcp/jsonrpc/testdata/single_server_notification_response.json",
            &normalized,
        );

        // Test multiple notifications in order
        let response = conn
            .handle_request(
                r#"{"jsonrpc": "2.0", "method": "trigger_multiple_notifications", "id": 2}"#
                    .to_string(),
            )
            .await
            .unwrap();

        // Verify notifications are received in order
        for i in 1..=3 {
            let notification = receiver.recv().await.expect("Should receive notification");
            let parsed: serde_json::Value = serde_json::from_str(&notification).unwrap();
            let normalized = serde_json::to_string_pretty(&parsed).unwrap();
            assert_contents(
                &format!(
                    "src/mcp/jsonrpc/testdata/multiple_server_notification_{}.json",
                    i
                ),
                &normalized,
            );
        }

        // Parse and normalize the response JSON
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        let normalized = serde_json::to_string_pretty(&parsed).unwrap();
        assert_contents(
            "src/mcp/jsonrpc/testdata/multiple_server_notifications_response.json",
            &normalized,
        );

        // Test parameterless notification
        let response = conn
            .handle_request(
                r#"{"jsonrpc": "2.0", "method": "trigger_parameterless_notification", "id": 3}"#
                    .to_string(),
            )
            .await
            .unwrap();
        let notification = receiver.recv().await.expect("Should receive notification");

        // Parse and normalize the notification JSON
        let parsed: serde_json::Value = serde_json::from_str(&notification).unwrap();
        let normalized = serde_json::to_string_pretty(&parsed).unwrap();
        assert_contents(
            "src/mcp/jsonrpc/testdata/parameterless_server_notification.json",
            &normalized,
        );

        // Parse and normalize the response JSON
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        let normalized = serde_json::to_string_pretty(&parsed).unwrap();
        assert_contents(
            "src/mcp/jsonrpc/testdata/parameterless_server_notification_response.json",
            &normalized,
        );
    }
}
