//! JSON RPC implementation that's specific to JSON RPC servers
#![allow(dead_code)] // TODO: Remove after impl is done
use super::shared as jsonrpc;
use downcast_rs::Downcast;
use dyn_clone::DynClone;
use std::any::Any;
use std::borrow::Cow;
use std::fmt::Debug;
use tracing::*;

/// Marker trait that is auto-implemented for all types that implement `Any` and `Clone`
///
/// This allows them to be boxed and used as [`BackgroundEvent`]s.
///
/// This is automatically implemented for all suitable types; callers should never have to interact
/// with this trait.
///
/// TODO: `Debug` is just to let us `unwrap()` in tests.  Maybe make that required only in builds
/// with tests enabled.
#[doc(hidden)]
pub(crate) trait BroadcastEventT:
    Downcast + Debug + Any + DynClone + Send + 'static
{
}
downcast_rs::impl_downcast!(BroadcastEventT);

impl<T> BroadcastEventT for T where T: Debug + Any + DynClone + Send + 'static {}

/// The type that is sent to all connection handlers and the service itself when a broadcast event
/// is sent.
///
/// This is as close to runtime polymorphism as we can get in Rust.
///
/// In order to be usable as a broadcast event, a type must be `Clone` and `Send`.  Technically it
/// must also implement `Any`, but that is implemented automatically.
pub(crate) type BroadcastEvent = Box<dyn BroadcastEventT>;

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
    ) -> Result<Self::ConnectionHandler, jsonrpc::JsonRpcError>;

    /// Handle a broadcast event sent to all clients.
    ///
    /// In most cases handling this will be specific to the actual active connection, and not the
    /// entire server, however the server is also given a chance to respond to the event.
    async fn handle_broadcast_event(
        &self,
        event: BroadcastEvent,
    ) -> Result<(), jsonrpc::JsonRpcError> {
        let _ = event;

        Ok(())
    }
}

/// The handler for a single JSON-RPC connection.
///
/// Mainly this is responsible for handling incoming method calls and notifications, but the
/// implementation can also hold connection-specific state since there is one connection handler
/// per connection.
///
/// Note that this handler will be cloned for each request received, so that requests can be
/// handled as parallel async tasks.  Implementors of this trait should ensure that the clone
/// operation is cheap, using an `Arc<Mutex<T>>` pattern when necessary.
#[async_trait::async_trait]
pub(crate) trait JsonRpcConnectionHandler: Clone + Send + Sync + 'static {
    /// Handle a JSON-RPC method invoation
    async fn handle_method<'a>(
        &self,
        request: MethodRequest<'a>,
    ) -> Result<serde_json::Value, jsonrpc::JsonRpcError> {
        // By default, no methods are supported
        warn!("Default trait impl rejects all method invocations.  Implement `handle_method` to override");

        Err(jsonrpc::JsonRpcError::method_not_found(
            request.method().to_string(),
            request.id().into_owned(),
        ))
    }

    /// Handle a JSON-RPC notification (which is like a method invocation, but no response is
    /// expected).
    ///
    /// Note that this operation is fallible only so that the server implementation can properly
    /// log errors handling notifications.  No error will be returned to the client because
    /// according to the JSON RPC spec servers MUST NOT return any response to notifications
    async fn handle_notification<'a>(
        &self,
        request: NotificationRequest<'a>,
    ) -> Result<(), jsonrpc::JsonRpcError> {
        // TODO: From the JSON RPC spec, it seems like it's valid to invoke any method without an
        // ID and thereby turn it into a notification.  Meaning, it's a fire-and-forget method
        // call.  Is that the case?  If so then this should default to calling `handle_method`
        // albeit without any ID.  But is that a real use case?  Whatever result that method call
        // produces is not going to be returned to the client, so it seems like it would be a waste
        // unless the purpose of the method is to permute state...

        // By default, no notifications are supported
        let _ = request;
        warn!("Default trait impl rejects all notifications.  Implement `handle_notification` to override");
        Ok(())
    }

    async fn handle_broadcast_event(&self, event: String) {
        let _ = event;
        warn!("Default trait impl ignores all broadcast events.  Implement `handle_broadcast_event` to override");
    }
}

/// Context specific to a single JSON RPC method call request
#[derive(Clone, Debug)]
pub(crate) struct MethodContext {
    abort_token: tokio_util::sync::CancellationToken,

    // TODO: flesh out.  Remember that progress has to be explicitly requested by the client or
    // progress updates should not be generated.
    progress_sender: tokio::sync::mpsc::Sender<()>,
}

/// A request from a client to invoke a method over JSON RPC
#[derive(Clone, Debug)]
pub(crate) struct MethodRequest<'a> {
    context: MethodContext,

    id: jsonrpc::Id<'a>,
    method: Cow<'a, str>,
    params: Option<Cow<'a, serde_json::value::RawValue>>,
}

impl<'a> MethodRequest<'a> {
    /// Given an input [`jsonrpc::Request`], bundle it into a MethodRequest for passing to
    /// handlers.
    fn from_jsonrpc_request(context: MethodContext, request: jsonrpc::Request<'a>) -> Self {
        let jsonrpc::Request {
            id, method, params, ..
        } = request;

        MethodRequest::<'_> {
            context,
            id,
            method,
            params,
        }
    }

    pub fn context(&self) -> &MethodContext {
        &self.context
    }

    pub fn id(&self) -> jsonrpc::Id<'a> {
        self.id.clone()
    }

    pub fn method(&self) -> &str {
        self.method.as_ref()
    }

    pub fn params(&'a self) -> Option<&'a serde_json::value::RawValue> {
        // It's not clear why jsonrpsee `jsonrpc::Request` structs use `Option<Cow<RawValue>>` while the
        // jsonrpc::Notification uses `Option<&RawValue>`.  For consistency, both the method and
        // notification handlers just take `Option<&RawValue>`, so jump through some hoops to hide
        // the Cow here.
        self.params.as_ref().map(|cow| cow.as_ref())
    }

    /// Attempt to decode the parameters of the request into a Rust type
    ///
    /// If this fails, a friendly JSON RPC error is produced suitable for returning directly to the
    /// client
    pub fn expect_params<P: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<P, jsonrpc::JsonRpcError> {
        expect_params(&self.id, self.params())
    }
}

/// A notification sent from a client
#[derive(Clone, Debug)]
pub(crate) struct NotificationRequest<'a> {
    method: Cow<'a, str>,
    params: Option<&'a serde_json::value::RawValue>,
}

impl<'a> NotificationRequest<'a> {
    fn from_jsonrpc_notification(notification: jsonrpc::Notification<'a>) -> Self {
        let jsonrpc::Notification { method, params, .. } = notification;

        NotificationRequest { method, params }
    }

    pub fn method(&self) -> &str {
        self.method.as_ref()
    }

    pub fn params(&self) -> Option<&serde_json::value::RawValue> {
        self.params
    }

    /// Attempt to decode the parameters of the request into a Rust type
    ///
    /// If this fails, a friendly JSON RPC error is produced suitable for returning directly to the
    /// client
    pub fn expect_params<P: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<P, jsonrpc::JsonRpcError> {
        expect_params(&jsonrpc::Id::Null, self.params.clone())
    }
}

/// Context for a single JSON-RPC connection.
///
/// TODO: fill this out.
#[derive(Clone, Debug)]
pub(crate) struct ConnectionContext {
    /// Channel to send notifications to the client connected to this connection
    server_notification_sender: ConnectionNotificationSender,
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

#[derive(Clone, Debug)]
pub(crate) struct ServiceContext {
    server_broadcast_sender: BroadcastNotificationSender,
}

impl ServiceContext {
    /// Send a broadcast notification to all connection handlers (and the service itself)
    ///
    /// See [`BroadcastEvent`] for more information on how to create a broadcast event.
    ///
    /// Note that this operation is infallible.  Broadcast events can fail to send, if there are no
    /// receivers, however that is not considered an error and it only logged.
    ///
    /// The service itself as well as all active connection handler instances will have the ability
    /// to receive broadcast events; whether they actually do or not depends upon their
    /// implementation.
    pub(crate) async fn send_broadcast_event<T, EventT>(&self, event: T)
    where
        T: Into<EventT>,
        EventT: BroadcastEventT,
    {
        let event = Box::new(event.into());

        if self
            .server_broadcast_sender
            .send(BroadcastEventContainer(event))
            .is_err()
        {
            error!(
                event_type = std::any::type_name::<EventT>(),
                "Error while sending broadcast notification; notification was not sent"
            );
        }
    }
}

/// Abstraction for anything that creates a new instance of a JSON RPC service.
///
/// In almost all cases, the implementation on `FnOnce` is sufficient.
pub(crate) trait JsonRpcServiceConstructor: Send + 'static {
    /// The service that is being constructed
    type Service: JsonRpcService;

    /// The error type that can be returned when creating a new service, in case service creation
    /// is fallible.  Use `Infallible` if the service creation is infallible.`
    type Error;

    fn create_service(self, service_context: ServiceContext) -> Result<Self::Service, Self::Error>;
}

/// Convenience impl of `JsonRpcServiceConstructor` for any function that can create a service
impl<F, S, E> JsonRpcServiceConstructor for F
where
    F: FnOnce(ServiceContext) -> Result<S, E> + Send + 'static,
    S: JsonRpcService,
{
    type Service = S;
    type Error = E;

    fn create_service(self, service_context: ServiceContext) -> Result<S, E> {
        self(service_context)
    }
}

/// Runtime-configurable settings for the JSON RPC server itself
///
/// These are independent of the actual JSON RPC service being hosted, and also of the transport
/// used to expose the service.
#[derive(Clone, Debug)]
pub(crate) struct JsonRpcServerConfig {
    /// The maximum number of pending notifications that can be buffered for a single connection.
    ///
    /// This translates into the bound for the channel that is used to send notifications back to
    /// the client.
    ///
    /// Most implementations should not need to modify this.
    max_pending_notifications: usize,
}

impl Default for JsonRpcServerConfig {
    fn default() -> Self {
        Self {
            max_pending_notifications: 100,
        }
    }
}

/// JSON RPC server which implements the JSON RPC-specific plumbing, then invokes some
/// [`JsonRpcService`] trait impl to do the actual logic.
pub(crate) struct JsonRpcServer<S> {
    config: JsonRpcServerConfig,

    service: S,

    /// Channel that will receive broadcast notifications for all connected clients
    /// This actually isn't ever used directly, due to a peculiarity of how tokio broadcast
    /// channels work.  But when the server is first created, there are no connections, so
    /// something must keep at least once broadcast receiver alive or the whole channel is closed.
    broadcast_receiver: BroadcastNotificationReceiver,

    /// A clone of the sender that goes w/ `broadcast_receiver`.  Tokio broadcast channels work
    /// unlike the other kinds in that the receivers have to be obtained from the sender, so the
    /// sender is kept here so that each new connection handler can have its own broadcast receiver
    ///
    /// TODO: THIS IS NOT TRUE!  If there are no receivers, then send operations fail, but they
    /// will start to succeed again if any receivers are created with the `subscribe()` method.
    /// Rework this.
    broadcast_sender: BroadcastNotificationSender,
}

impl<S> JsonRpcServer<S>
where
    S: JsonRpcService,
{
    /// Create a new JSON-RPC server from a service constructor.
    ///
    /// Returns a tuple:
    /// - The JSON RPC server instance, ready to be hooked up to a transport and start handling
    ///   requests
    /// - A [`BroadcastNotificationReceiver`] that will receive notifications that are broadcast to
    ///   all connections by using [`ServiceContext::send_notification`].  This should also be
    ///   provided to the transport, as the handling of broadcast notifications is transport-specific.
    ///   TODO: Do we need to furnish a receiver here?  The transport can't do anything with events
    ///   except when it's connected to a client, so it seems like the fact that the
    ///   each connection handler is given a `NotificationReceiver` is sufficient.
    pub(crate) fn from_service<Constructor>(
        config: JsonRpcServerConfig,
        ctor: Constructor,
    ) -> Result<
        (
            JsonRpcServer<Constructor::Service>,
            BroadcastNotificationReceiver,
        ),
        Constructor::Error,
    >
    where
        Constructor: JsonRpcServiceConstructor<Service = S>,
    {
        let bound = config.max_pending_notifications;
        let (broadcast_sender, broadcast_receiver) =
            tokio::sync::broadcast::channel::<BroadcastEventContainer>(bound);
        let service = ctor.create_service(ServiceContext {
            server_broadcast_sender: broadcast_sender.clone(),
        })?;

        Ok((
            Self {
                config,
                service,
                broadcast_receiver: broadcast_sender.subscribe(),
                broadcast_sender,
            },
            broadcast_receiver,
        ))
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
        let (sender, receiver) = tokio::sync::mpsc::channel(self.config.max_pending_notifications);
        let context = ConnectionContext {
            server_notification_sender: sender.clone(),
        };
        let handler = self.service.handle_connection(context).await?;

        Ok((
            JsonRpcServerConnection::new(handler, sender),
            NotificationReceiver::new(receiver, self.broadcast_sender.subscribe()),
        ))
    }
}

/// A JSON-RPC connection that is specific to JSON-RPC servers.
pub(crate) struct JsonRpcServerConnection<H> {
    handler: H,
    server_notification_sender: ConnectionNotificationSender,
}

impl<H> JsonRpcServerConnection<H>
where
    H: JsonRpcConnectionHandler,
{
    fn new(handler: H, server_notification_sender: ConnectionNotificationSender) -> Self {
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
        let context: MethodContext = todo!();
        let request = MethodRequest::from_jsonrpc_request(context, request);

        self.handler.handle_method(request).await.inspect_err(|e| {
            error!(error = ?e, "Error handling method invocation");
        })
    }

    /// Handle a JSON-RPC notification (which is like a method invocation, but no response is
    /// expected).
    ///
    /// Note that this operation is fallible only so that the server implementation can log errors
    /// handling notifications if it is interested in doing so.  No error will be returned to the
    /// client because according to the JSON RPC spec servers MUST NOT return any response to
    /// notifications
    #[instrument(skip_all, fields(method = %request.method))]
    async fn handle_notification<'a>(
        &self,
        request: jsonrpc::Notification<'a>,
    ) -> Result<(), jsonrpc::JsonRpcError> {
        let request = NotificationRequest::from_jsonrpc_notification(request);

        if let Err(e) = self.handler.handle_notification(request).await {
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

/// Newtype hack to allow for a `Box<dyn BroadcastEventT>` to be cloned
struct BroadcastEventContainer(Box<dyn BroadcastEventT>);

impl BroadcastEventContainer {
    fn new(event: impl BroadcastEventT) -> Self {
        Self(Box::new(event))
    }
}

impl Clone for BroadcastEventContainer {
    fn clone(&self) -> Self {
        Self(dyn_clone::clone_box(&*self.0))
    }
}

impl std::fmt::Debug for BroadcastEventContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BroadcastEvent")
            .field(&self.0.type_id())
            .finish()
    }
}

type ConnectionNotificationSender = tokio::sync::mpsc::Sender<String>;

/// Channel receiver for notifications sent asynchronously from the server, to the client, but not
/// in response to a specific client request.
type ConnectionNotificationReceiver = tokio::sync::mpsc::Receiver<String>;

type BroadcastNotificationSender = tokio::sync::broadcast::Sender<BroadcastEventContainer>;

/// The counterpart to [`NotificationReceiver`] that is used to receive notifications that are
/// broadcast to all clients.
type BroadcastNotificationReceiver = tokio::sync::broadcast::Receiver<BroadcastEventContainer>;

/// The type of notification received from either the connection-specific channel or the broadcast channel.
pub(crate) enum NotificationKind {
    /// A notification that was broadcast to all clients
    Broadcast(BroadcastEvent),
    /// A notification that was sent specifically to this client
    Connection(String),
    /// Indicates that some broadcast messages were missed due to the receiver falling behind
    Lagged(u64),
}

impl std::fmt::Debug for NotificationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotificationKind::Broadcast(event) => {
                f.debug_tuple("Broadcast").field(&event.type_id()).finish()
            }
            NotificationKind::Connection(notification) => {
                f.debug_tuple("Connection").field(notification).finish()
            }
            NotificationKind::Lagged(n) => f.debug_tuple("Lagged").field(n).finish(),
        }
    }
}

/// Handle which has access to the notification channels for both a specific connection and the
/// broadcast channel for all connections.
///
/// When polled using its [`Self::recv`] function, it will yield the next notification, either
/// from the server's connection handler for the specific connection that this receiver is
/// associated with, or from the broadcast channel.
///
/// # Note
///
/// Because this pulls from both the broadcast receiver and the connection receiver, it can
/// possibly return a broadcast notification even after the connection (and the associated
/// connection notification sender) has closed.  However it's unlikely that there would be very
/// many such notifications because this receiver detects that the connection's notification
/// channel is closed (indicating that the async process that produces channel notifications has
/// terminated), and subsequently returns `None`.
#[derive(Debug)]
pub(crate) struct NotificationReceiver {
    connection_receiver: Option<ConnectionNotificationReceiver>,
    broadcast_receiver: Option<BroadcastNotificationReceiver>,
}

impl NotificationReceiver {
    fn new(
        connection_receiver: ConnectionNotificationReceiver,
        broadcast_receiver: BroadcastNotificationReceiver,
    ) -> Self {
        Self {
            connection_receiver: Some(connection_receiver),
            broadcast_receiver: Some(broadcast_receiver),
        }
    }

    /// Waits for the next notification from either the connection-specific channel or the broadcast
    /// channel that is shared by all connections in the service.
    ///
    /// Returns `None` once the connection-specific channel is closed, which means no more
    /// connection-specific events will be received, which usually means that the connection has
    /// been closed by the transport.
    pub async fn recv(&mut self) -> Option<NotificationKind> {
        loop {
            match (
                self.connection_receiver.as_mut(),
                self.broadcast_receiver.as_mut(),
            ) {
                (None, _) => {
                    // The connection channel is closed, so no more notifications should be
                    // furnished to this connection, even though the broadcast channel is likely
                    // still around
                    return None;
                }
                (Some(conn), None) => {
                    if let Some(notification) = conn.recv().await {
                        return Some(NotificationKind::Connection(notification));
                    } else {
                        // This connection is closed, so don't poll it anymore
                        self.connection_receiver = None;
                    }
                }
                (Some(conn), Some(broadcast)) => {
                    // Both recv futures should be cancel-safe, so just return the first one that
                    // resolves

                    tokio::select! {
                        // Try to receive from the connection channel
                        result = conn.recv() => {
                            if let Some(notification) = result {
                                return Some(NotificationKind::Connection(notification));
                            } else {
                                // This connection is closed, so don't poll it anymore
                                self.connection_receiver = None;
                            }
                        },
                        // Try to receive from the broadcast channel
                        broadcast_result = broadcast.recv() => {
                            match broadcast_result {
                                Ok(notification) => return Some(NotificationKind::Broadcast(notification.0)),
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    warn!(missed_messages = n, "Broadcast receiver for this connection fell behind and missed some broadcast messages");
                                    return Some(NotificationKind::Lagged(n))
                                },
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    // The broadcast channel is closed, so don't poll it anymore
                                    self.broadcast_receiver = None;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use expectorate::assert_contents;
    use serde_json::json;
    use std::collections::HashSet;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio::sync::mpsc;

    // Mock connection handler for testing
    #[derive(Clone)]
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
            request: MethodRequest<'a>,
        ) -> Result<serde_json::Value, jsonrpc::JsonRpcError> {
            match request.method().as_ref() {
                "echo" => {
                    let params = request.expect_params::<serde_json::Value>()?;
                    Ok(params)
                }
                "add" => {
                    #[derive(serde::Deserialize)]
                    struct AddParams {
                        a: i32,
                        b: i32,
                    }
                    let params = request.expect_params::<AddParams>()?;
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
                    request.id().into_owned(),
                    anyhow::anyhow!("Test error!"),
                )),
                unknown => Err(jsonrpc::JsonRpcError::method_not_found(
                    unknown.to_string(),
                    request.id().into_owned(),
                )),
            }
        }

        async fn handle_notification<'a>(
            &self,
            request: NotificationRequest<'a>,
        ) -> Result<(), jsonrpc::JsonRpcError> {
            match request.method().as_ref() {
                "notify" => Ok(()),
                unknown => Err(jsonrpc::JsonRpcError::method_not_found(
                    unknown.to_string(),
                    jsonrpc::Id::Null,
                )),
            }
        }
    }

    // Helper function to create a server connection for testing
    fn create_test_connection() -> (
        JsonRpcServerConnection<MockHandler>,
        ConnectionNotificationReceiver,
    ) {
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_notification_receiver() {
        use tokio::sync::broadcast;
        use tokio::sync::mpsc;

        // Create channels
        let (conn_tx, conn_rx) = mpsc::channel(10);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(10);

        let mut receiver = NotificationReceiver::new(conn_rx, broadcast_rx);

        // Test connection message
        conn_tx.send("conn_msg".to_string()).await.unwrap();
        assert_matches!(
            receiver.recv().await,
            Some(NotificationKind::Connection(msg)) if msg == "conn_msg"
        );

        // Test broadcast message
        broadcast_tx
            .send(BroadcastEventContainer::new("broadcast_msg".to_string()))
            .unwrap();
        assert_matches!(
            receiver.recv().await,
            Some(NotificationKind::Broadcast(event)) => {
                assert_eq!("broadcast_msg", event.downcast::<String>().unwrap().as_str());
            }
        );

        // Test lagged broadcast messages
        // According to the docs, lagging starts at the first integer power of 2 greater than the
        // channel bound.  Channel bound is 10 so that means lagging starts at 16
        for i in 0..17 {
            broadcast_tx
                .send(BroadcastEventContainer::new(format!("msg{}", i)))
                .unwrap();
        }
        // Should get the Lagged notification
        match receiver.recv().await {
            Some(NotificationKind::Lagged(n)) => assert_eq!(n, 1), // We missed 1 message
            other => panic!("Expected Lagged notification, got {:?}", other),
        }
        // We should receive all of the messages we sent, except for the oldest that we missed
        for i in 1..17 {
            let msg = dbg!(receiver.recv().await.unwrap());
            assert!(matches!(msg, NotificationKind::Broadcast(_)));
            if let NotificationKind::Broadcast(content) = msg {
                assert_eq!(
                    format!("msg{i}"),
                    content.downcast::<String>().unwrap().as_str()
                );
            }
        }

        // Test closing connection sender
        drop(conn_tx);
        // Broadcast channel still open, should still receive broadcast messages
        broadcast_tx
            .send(BroadcastEventContainer::new("after_conn_close".to_string()))
            .unwrap();
        assert_matches!(
            receiver.recv().await,
            Some(NotificationKind::Broadcast(content)) => {
                assert_eq!("after_conn_close",content.downcast::<String>().unwrap().as_str());
            }
        );

        // Test closing broadcast sender
        drop(broadcast_tx);
        // Both channels closed, should return None
        assert_matches!(receiver.recv().await, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_notification_receiver_concurrent() {
        use tokio::time::{sleep, Duration};

        // Create channels
        let (conn_tx, conn_rx) = mpsc::channel(10);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(10);

        let mut receiver = NotificationReceiver::new(conn_rx, broadcast_rx);

        // Spawn a task that sends connection messages after a delay
        let conn_handle = tokio::spawn({
            let conn_tx = conn_tx.clone();
            async move {
                sleep(Duration::from_millis(50)).await;
                conn_tx.send("delayed_conn".to_string()).await.unwrap();
            }
        });

        // Spawn a task that sends broadcast messages after a different delay
        let broadcast_handle = tokio::spawn({
            let broadcast_tx = broadcast_tx.clone();
            async move {
                sleep(Duration::from_millis(25)).await;
                broadcast_tx
                    .send(BroadcastEventContainer::new(
                        "delayed_broadcast".to_string(),
                    ))
                    .unwrap();
            }
        });

        // First message should be the broadcast message since it has a shorter delay
        assert_matches!(
            receiver.recv().await,
            Some(NotificationKind::Broadcast(content)) => {
                assert_eq!(
        "delayed_broadcast",
                    content.downcast::<String>().unwrap().as_str()
            );
            }
        );
        // Second message should be the connection message
        assert_matches!(
            receiver.recv().await,
            Some(NotificationKind::Connection(content)) => {
                assert_eq!("delayed_conn", &content);
            }
        );

        // Wait for spawned tasks to complete
        conn_handle.await.unwrap();
        broadcast_handle.await.unwrap();

        // Clean up
        drop(conn_tx);
        drop(broadcast_tx);
        assert_matches!(receiver.recv().await, None);
    }

    /// Slam the notification receiver with a bunch of messages from multiple connection and
    /// broadcast senders.
    ///
    /// The order in this case is non-deterministic, but the test is to make sure that nothing gets
    /// missed.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_notification_receiver_stress() {
        const SENDERS: &[&str] = &[
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliet",
        ];

        fn generate_connection_messages(sender: &'static str) -> Vec<String> {
            (0..100).map(|i| format!("{sender}-conn-{i}")).collect()
        }

        fn generate_broadcast_messages(sender: &'static str) -> Vec<String> {
            (0..100)
                .map(|i| format!("{sender}-broadcast-{i}"))
                .collect()
        }

        let gen_conn = |sender| generate_connection_messages(sender);
        let gen_broad = |sender| generate_broadcast_messages(sender);

        // Create channels
        let (conn_tx, conn_rx) = tokio::sync::mpsc::channel(10);
        let (broadcast_tx, broadcast_rx) =
            tokio::sync::broadcast::channel::<BroadcastEventContainer>(10);

        // Create (but don't yet start to poll) the futures that will send messages
        let senders = SENDERS.iter().map(move |sender| {
            tokio::spawn({
                use rand::prelude::*;

                let connection_messages = gen_conn(sender);
                let broadcast_messages = gen_broad(sender);
                let conn_tx = conn_tx.clone();
                let broadcast_tx = broadcast_tx.clone();
                let send_interval = Duration::from_millis(thread_rng().gen_range(1..10));

                async move {
                    for (conn_msg, broad_msg) in
                        connection_messages.into_iter().zip(broadcast_messages)
                    {
                        tokio::time::sleep(send_interval).await;
                        conn_tx.send(conn_msg).await.unwrap();
                        tokio::time::sleep(send_interval).await;
                        broadcast_tx
                            .send(BroadcastEventContainer::new(broad_msg))
                            .unwrap();
                    }
                }
            })
        });

        let mut receiver = NotificationReceiver::new(conn_rx, broadcast_rx);

        let mut conn_messages = HashSet::new();
        let mut broad_messages = HashSet::new();

        // Poll all of the senders
        let senders = tokio::spawn(async move {
            futures::future::join_all(senders).await;
        });

        while let Some(notification) = receiver.recv().await {
            match notification {
                NotificationKind::Connection(msg) => {
                    assert!(
                        conn_messages.insert(msg.clone()),
                        "Duplicate connection message: {}",
                        msg
                    );
                }
                NotificationKind::Broadcast(msg) => {
                    let msg = msg.downcast::<String>().unwrap();
                    assert!(
                        broad_messages.insert(msg.clone()),
                        "Duplicate broadcast message: {}",
                        msg
                    );
                }
                NotificationKind::Lagged(n) => {
                    // In this test this should not happen
                    panic!("Bug in the test: {n} lagged messages");
                }
            }
        }

        senders.await.unwrap();

        // Make sure that all of the messages from all of the senders were received
        for sender in SENDERS {
            for msg in generate_connection_messages(sender) {
                assert!(
                    conn_messages.contains(&msg),
                    "Missing connection message: {}",
                    msg
                );
            }

            for msg in generate_broadcast_messages(sender) {
                assert!(
                    broad_messages.contains(&msg),
                    "Missing broadcast message: {}",
                    msg
                );
            }
        }
    }

    #[tokio::test]
    async fn test_custom_broadcast_event_types() {
        let (_conn_tx, conn_rx) = mpsc::channel(10);
        let (broadcast_tx, broadcast_rx) = broadcast::channel(10);

        let mut receiver = NotificationReceiver::new(conn_rx, broadcast_rx);

        #[derive(Clone, Debug)]
        struct CustomBroadcastEvent {
            message: String,
        }

        broadcast_tx
            .send(BroadcastEventContainer::new(CustomBroadcastEvent {
                message: "Hello, world!".to_string(),
            }))
            .unwrap();

        assert_matches!(receiver.recv().await, Some(NotificationKind::Broadcast(event)) => {
            let event = event.downcast::<CustomBroadcastEvent>().unwrap();
            assert_eq!("Hello, world!", event.message);
        });
    }
}
